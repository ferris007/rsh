//! Shell state, and the dispatch of a single line.

use std::io::Write;

use nix::errno::Errno;
use rsh_parser::{Command, ParseError, Pipeline, Span};
use rsh_process::{Redirections, ResolveError, Restore, EXIT_NOT_EXECUTABLE, EXIT_NOT_FOUND};

use crate::builtin::Builtin;
use crate::expand::{expand_all, ProcessEnv};
use crate::redirect::{plan, RedirectError};

/// Exit status for input the shell could not parse, or could not carry out.
///
/// POSIX shells reserve 2 for usage and syntax errors, which keeps it distinct
/// from a command that ran and failed.
const EXIT_SYNTAX: i32 = 2;

/// Whether the REPL should keep going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Prompt for another line.
    Continue,
    /// The shell was asked to exit with this status.
    Exit(i32),
}

/// The shell's live state.
///
/// Deliberately not the whole world: the current directory and environment are
/// process state owned by the kernel and libc, and duplicating them here would
/// create two sources of truth that drift the moment a builtin forgets to
/// update one. What lives here is what has no other home.
#[derive(Debug, Default)]
pub struct Shell {
    last_status: i32,
}

impl Shell {
    /// A shell with no history: the last status is 0, as at login.
    pub fn new() -> Self {
        Self::default()
    }

    /// The status of the most recent command — what `$?` reports.
    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    /// Install the shell's signal handlers.
    ///
    /// Exposed here rather than reached for directly by the binary, so the
    /// layering stays honest: the REPL talks to the executor, and the executor
    /// is what owns shell state — of which "has a Ctrl-C arrived" is a part.
    pub fn install_signal_handlers(&self) -> Result<(), Errno> {
        rsh_process::install_signal_handlers()
    }

    /// Whether a Ctrl-C has arrived since this was last called.
    pub fn take_interrupt(&self) -> bool {
        rsh_process::take_interrupt()
    }

    /// The signal number that asked the shell to shut down, if one has.
    ///
    /// A raw number rather than a signal type, so that the binary does not need
    /// to know about `nix` to ask the question.
    pub fn shutdown_requested(&self) -> Option<i32> {
        rsh_process::shutdown_requested().map(|signal| signal as i32)
    }

    /// Record a status that did not come from running a command.
    ///
    /// Ctrl-C at the prompt is the case: nothing ran, but `$?` should report
    /// 130 as it would in any other shell, so that a later `echo $?` tells the
    /// truth about what happened.
    pub fn set_last_status(&mut self, status: i32) {
        self.last_status = status;
    }

    /// Parse and run one line of input.
    ///
    /// Errors are reported to stderr as they happen rather than returned: a
    /// shell's job is to tell the user what went wrong and carry on, and a
    /// failed command is a normal event, not an exceptional one.
    pub fn run_line(&mut self, line: &str) -> Outcome {
        let pipeline = match rsh_parser::parse(line) {
            Ok(Some(pipeline)) => pipeline,
            // A blank line is not a command and does not disturb `$?`. Typing
            // Enter at a prompt should never change what `$?` reports.
            Ok(None) => return Outcome::Continue,
            Err(error) => {
                self.report_parse_error(line, &error);
                return Outcome::Continue;
            }
        };

        self.run_pipeline(line, &pipeline)
    }

    fn run_pipeline(&mut self, line: &str, pipeline: &Pipeline) -> Outcome {
        if pipeline.commands().len() > 1 {
            let env = ProcessEnv::new(self.last_status);
            self.last_status = match crate::pipeline::run(pipeline, &env) {
                Ok(status) => status,
                Err(error) => {
                    eprintln!("rsh: {error}");
                    1
                }
            };
            return Outcome::Continue;
        }

        self.run_command(line, &pipeline.commands()[0])
    }

    fn run_command(&mut self, line: &str, command: &Command) -> Outcome {
        let env = ProcessEnv::new(self.last_status);
        let argv = expand_all(command.words(), &env);

        // Redirections are set up before the command is looked up, which is why
        // `nosuchcmd > out` still creates `out`. Every POSIX shell behaves this
        // way, and scripts rely on it: `> lockfile` is a command whose only
        // effect is its redirection.
        let redirections = match plan(command, &env) {
            Ok(plan) => plan,
            Err(error) => {
                self.report_redirect_error(line, &error);
                return Outcome::Continue;
            }
        };

        // Everything expanded away: `$UNSET` on its own. POSIX says a command
        // with no name and no assignments completes with status zero, so this
        // is a successful no-op rather than an error.
        let Some(program) = argv.first() else {
            self.last_status = 0;
            return Outcome::Continue;
        };

        match Builtin::lookup(program) {
            Some(builtin) => {
                // A builtin runs inside the shell, so there is no child whose
                // descriptors can be arranged. The shell moves its own and puts
                // them back — which is what `Restore` exists for.
                let _restore = match apply_to_shell(&redirections) {
                    Ok(restore) => restore,
                    Err(error) => {
                        eprintln!("rsh: {error}");
                        self.last_status = 1;
                        return Outcome::Continue;
                    }
                };

                let (outcome, status) = builtin.run(&argv[1..], self.last_status);

                // Flush while the redirection is still in place. Anything left
                // in the buffer would otherwise be written after the descriptor
                // has been put back, and land on the terminal instead of in the
                // file.
                let _ = std::io::stdout().flush();

                self.last_status = status;
                outcome
            }
            None => {
                self.last_status = run_external(&argv, redirections);
                Outcome::Continue
            }
        }
    }

    /// Report a parse error, underlining the characters at fault.
    fn report_parse_error(&mut self, line: &str, error: &ParseError) {
        eprintln!("rsh: {error}");
        underline(line, error.span());
        self.last_status = EXIT_SYNTAX;
    }

    /// Report a redirection that could not be set up.
    ///
    /// Status 1, not 2: the line was valid, and the failure is about the state
    /// of the filesystem rather than about what the user wrote.
    fn report_redirect_error(&mut self, line: &str, error: &RedirectError) {
        eprintln!("rsh: {error}");
        underline(line, error.span());
        self.last_status = 1;
    }
}

/// Apply redirections to the shell itself, for a builtin.
///
/// Stdout is flushed first: anything still buffered was produced before the
/// redirection and belongs on the old descriptor.
fn apply_to_shell(redirections: &Redirections) -> Result<Option<Restore>, Errno> {
    if redirections.is_empty() {
        return Ok(None);
    }

    let _ = std::io::stdout().flush();
    redirections.apply_scoped().map(Some)
}

/// Run a command as a child process, and wait for it.
fn run_external(argv: &[String], redirections: Redirections) -> i32 {
    let program = &argv[0];

    let path = match rsh_process::resolve(program, std::env::var_os("PATH").as_deref()) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("rsh: {error}");
            return match error {
                ResolveError::NotFound { .. } => EXIT_NOT_FOUND,
                ResolveError::NotExecutable { .. } => EXIT_NOT_EXECUTABLE,
            };
        }
    };

    let prepared = match rsh_process::Command::new(&path, argv).map(|command| {
        // Attached now, applied in the child. The shell's own descriptors are
        // never touched — which is why `echo hi > f` leaves the prompt where it
        // was, and why a child cannot redirect its parent.
        command.redirections(redirections)
    }) {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("rsh: {error}");
            return EXIT_NOT_EXECUTABLE;
        }
    };

    // Flush before forking. The child inherits a copy of this buffer, and
    // anything still sitting in it would be written twice: once by us, once by
    // the child when it exits. See experiments/fork_exec.
    let _ = std::io::stdout().flush();

    let waited = prepared.spawn().and_then(|child| {
        child.wait_with(|signal| {
            // The user pressed Ctrl-Z, or the program stopped itself. There is
            // nowhere to put a stopped job until there is a job table, so the
            // shell says what it is doing rather than appearing to ignore the
            // keystroke.
            eprintln!("rsh: {program}: stopped by {signal}; continuing (job control is phase 6)");
        })
    });

    match waited {
        Ok(status) => status.code(),
        Err(error) => {
            eprintln!("rsh: {program}: {error}");
            EXIT_NOT_EXECUTABLE
        }
    }
}

/// Print the offending line with a caret under the span.
///
/// This is what the parser's spans are for. "syntax error near unexpected
/// token" is what a shell says when it has thrown this information away.
fn underline(line: &str, span: Span) {
    let line = line.trim_end();
    let start = span.start.min(line.len());
    let end = span.end.clamp(start, line.len());

    // Counted in characters, not bytes: a caret positioned by byte offset lands
    // in the wrong column as soon as the line contains anything non-ASCII.
    let column = line[..start].chars().count();
    let width = line[start..end].chars().count().max(1);

    eprintln!("  {line}");
    eprintln!("  {}{}", " ".repeat(column), "^".repeat(width));
}
