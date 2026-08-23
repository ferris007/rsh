//! Shell state, and the dispatch of a single line.

use std::io::Write;

use rsh_parser::{Command, ParseError, Pipeline, Span};
use rsh_process::{ResolveError, EXIT_NOT_EXECUTABLE, EXIT_NOT_FOUND};

use crate::builtin::Builtin;
use crate::expand::{expand_all, ProcessEnv};

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
        // Phase 2 builds the whole tree but can still only run one process.
        // Reporting that here rather than refusing to parse is the point of
        // this phase: the shell now understands the shape of what it declines.
        if let [first, second, ..] = pipeline.commands() {
            let span = operator_between(line, first.span().end, second.span().start, '|');
            self.refuse(line, span, "pipelines are not implemented yet", 4);
            return Outcome::Continue;
        }

        let command = &pipeline.commands()[0];

        if let Some(redirect) = command.redirects().first() {
            self.refuse(
                line,
                redirect.span(),
                "redirection is not implemented yet",
                3,
            );
            return Outcome::Continue;
        }

        self.run_command(command)
    }

    fn run_command(&mut self, command: &Command) -> Outcome {
        let env = ProcessEnv::new(self.last_status);
        let argv = expand_all(command.words(), &env);

        // Everything expanded away: `$UNSET` on its own. POSIX says a command
        // with no name and no assignments completes with status zero, so this
        // is a successful no-op rather than an error.
        let Some(program) = argv.first() else {
            self.last_status = 0;
            return Outcome::Continue;
        };

        match Builtin::lookup(program) {
            Some(builtin) => {
                let (outcome, status) = builtin.run(&argv[1..], self.last_status);
                self.last_status = status;
                outcome
            }
            None => {
                self.last_status = run_external(&argv);
                Outcome::Continue
            }
        }
    }

    /// Report syntax the shell understands but has not implemented.
    fn refuse(&mut self, line: &str, span: Span, message: &str, phase: u8) {
        eprintln!("rsh: {message} (roadmap phase {phase})");
        underline(line, span);
        self.last_status = EXIT_SYNTAX;
    }

    /// Report a parse error, underlining the characters at fault.
    fn report_parse_error(&mut self, line: &str, error: &ParseError) {
        eprintln!("rsh: {error}");
        underline(line, error.span());
        self.last_status = EXIT_SYNTAX;
    }
}

/// Run a command as a child process, and wait for it.
fn run_external(argv: &[String]) -> i32 {
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

    let prepared = match rsh_process::Command::new(&path, argv) {
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

    match prepared.spawn().and_then(rsh_process::Child::wait) {
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

/// Find an operator character between two commands, for pointing at it.
///
/// Falls back to the whole gap if the character is not there, which cannot
/// happen for a parsed pipeline but keeps this total rather than panicking on a
/// future caller's behalf.
fn operator_between(line: &str, from: usize, to: usize, operator: char) -> Span {
    let gap = line.get(from..to).unwrap_or("");
    match gap.find(operator) {
        Some(offset) => Span::new(from + offset, from + offset + operator.len_utf8()),
        None => Span::new(from, to),
    }
}
