//! Shell state, and the dispatch of a single line.

use std::io::Write;

use nix::errno::Errno;
use nix::unistd::{getpgrp, Pid};
use rsh_job::JobTable;
use rsh_parser::{Command, ParseError, Pipeline, Span};
use rsh_process::{Redirections, ResolveError, Restore, EXIT_NOT_EXECUTABLE, EXIT_NOT_FOUND};

use crate::builtin::{Builtin, Context as BuiltinContext};
use crate::expand::{expand_all, ProcessEnv};
use crate::pipeline::Context as PipelineContext;
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
#[derive(Debug)]
pub struct Shell {
    last_status: i32,
    jobs: JobTable,
    /// Whether the shell owns a terminal and can therefore do job control.
    ///
    /// Decided once, at startup. A shell reading a script has no terminal to
    /// hand over and nobody to type Ctrl-Z, so it runs children in its own
    /// process group exactly as it did before job control existed.
    job_control: bool,
    /// The shell's own process group, to take the terminal back from a job.
    shell_pgid: Pid,
    /// Whether the user has already been warned about stopped jobs.
    warned_about_jobs: bool,
    /// The terminal settings the shell wants for its own prompt.
    ///
    /// Captured once, at startup, before any job has had a chance to change
    /// them. Restored after every foreground job.
    terminal_modes: Option<rsh_terminal::Modes>,
    /// Whether `COLUMNS` and `LINES` have been set at least once.
    window_size_known: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Self {
            last_status: 0,
            jobs: JobTable::new(),
            job_control: rsh_terminal::is_interactive(),
            shell_pgid: getpgrp(),
            warned_about_jobs: false,
            terminal_modes: rsh_terminal::snapshot(),
            window_size_known: false,
        }
    }
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

    /// Keep `COLUMNS` and `LINES` in step with the window.
    ///
    /// These are environment variables, so children read them — a shell that
    /// never updated them would hand every program it runs a stale idea of the
    /// window, and `less` or `ps` would format for the wrong width.
    ///
    /// Called at startup and after every `SIGWINCH`. The handler itself only
    /// sets a flag: `ioctl` is not something a signal handler may call, and
    /// there is no hurry, because the answer is only useful at the prompt.
    pub fn refresh_window_size(&mut self) {
        if !self.window_size_known || rsh_process::take_resize() {
            if let Some(size) = rsh_terminal::size() {
                std::env::set_var("COLUMNS", size.cols.to_string());
                std::env::set_var("LINES", size.rows.to_string());
            }
            self.window_size_known = true;
        }
    }

    /// Put the terminal back the way the shell found it.
    ///
    /// Called on the way out. A shell that exits without doing this leaves
    /// whatever the last job did in place — and the last job may well have been
    /// killed halfway through changing it.
    pub fn restore_terminal(&self) {
        if let Some(modes) = &self.terminal_modes {
            let _ = rsh_terminal::restore(modes);
        }
    }

    /// Notice what background jobs have done, and say so.
    ///
    /// Called once per prompt rather than continuously. A shell that announced
    /// a finished job the instant it happened would write over whatever the
    /// user was typing; every shell waits for a quiet moment, and the top of
    /// the loop is the quiet moment.
    pub fn report_jobs(&mut self) {
        if !rsh_process::take_child_event() && self.jobs.is_empty() {
            return;
        }

        let events = rsh_process::collect_child_events();
        self.jobs.apply(&events);

        for job in self.jobs.take_reportable() {
            println!(
                "[{}]{}  {:<22}  {}",
                job.id(),
                self.jobs.marker(job.id()),
                job.state().describe(),
                job.command()
            );
        }

        self.jobs.forget_finished();
    }

    /// Whether any job is still running or stopped.
    pub fn has_jobs(&self) -> bool {
        self.jobs.iter().any(|job| job.state().is_alive())
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
        match self.dispatch(line) {
            Outcome::Exit(status) if !self.confirm_exit() => {
                // Warned, not exited. The next attempt goes through.
                let _ = status;
                Outcome::Continue
            }
            outcome => {
                self.warned_about_jobs = false;
                outcome
            }
        }
    }

    /// Whether it is all right to leave, warning once if it is not.
    ///
    /// Only *stopped* jobs earn a warning. A running background job carries on
    /// perfectly well without a shell; a stopped one would be left suspended
    /// with nothing able to resume it, which is a process leaked in a state the
    /// user cannot see. Warning once and letting the second attempt through is
    /// what every shell does, and it is the right shape: the shell states the
    /// consequence, and the decision stays with the user.
    pub fn confirm_exit(&mut self) -> bool {
        let stopped = self
            .jobs
            .iter()
            .any(|job| matches!(job.state(), rsh_job::JobState::Stopped));

        if stopped && !self.warned_about_jobs {
            eprintln!("rsh: there are stopped jobs");
            self.warned_about_jobs = true;
            return false;
        }

        true
    }

    fn dispatch(&mut self, line: &str) -> Outcome {
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
        // A single builtin runs in the shell itself. Everything else — one
        // command or ten — becomes a job, because with job control a lone
        // `sleep 30` needs a process group and the terminal exactly as much as
        // a pipeline does, and two spawning paths means two places to get that
        // wrong.
        if let [command] = pipeline.commands() {
            if let Some(word) = command.words().first() {
                let env = ProcessEnv::new(self.last_status);
                let argv = expand_all(std::slice::from_ref(word), &env);
                let is_builtin = argv
                    .first()
                    .is_some_and(|name| Builtin::lookup(name).is_some());

                if is_builtin && !pipeline.background() {
                    return self.run_command(line, command);
                }
            }
        }

        let env = ProcessEnv::new(self.last_status);
        let text = source_text(line, pipeline);
        let mut ctx = PipelineContext {
            jobs: &mut self.jobs,
            job_control: self.job_control,
            shell_pgid: self.shell_pgid,
            shell_modes: self.terminal_modes.as_ref(),
        };

        self.last_status = match crate::pipeline::run(pipeline, &env, &mut ctx, &text) {
            Ok(status) => status,
            Err(error) => {
                eprintln!("rsh: {error}");
                1
            }
        };

        Outcome::Continue
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

                let mut ctx = BuiltinContext {
                    jobs: &mut self.jobs,
                    job_control: self.job_control,
                    shell_pgid: self.shell_pgid,
                    shell_modes: self.terminal_modes.as_ref(),
                    last_status: self.last_status,
                };
                let (outcome, status) = builtin.run(&argv[1..], &mut ctx);

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

/// The text of a pipeline as the user wrote it, for the job table.
///
/// Taken from the source line rather than reconstructed from the AST: a job
/// listing should show what was typed, not the shell's idea of an equivalent
/// command. The trailing `&` is dropped because every listing already says the
/// job is in the background.
fn source_text(line: &str, pipeline: &Pipeline) -> String {
    let span = pipeline.span();
    let text = span.slice(line).unwrap_or(line).trim();
    text.strip_suffix('&').unwrap_or(text).trim_end().to_owned()
}
