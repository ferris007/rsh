//! Shell state, and the dispatch of a single line.

use std::io::Write;

use rsh_parser::Command;
use rsh_process::{ResolveError, EXIT_NOT_EXECUTABLE, EXIT_NOT_FOUND};

use crate::builtin::Builtin;

/// Exit status used for input the shell could not parse.
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

    /// The status of the most recent command — what `$?` will report once
    /// expansion exists (Phase 2).
    pub fn last_status(&self) -> i32 {
        self.last_status
    }

    /// Parse and run one line of input.
    ///
    /// Errors are reported to stderr as they happen rather than returned: a
    /// shell's job is to tell the user what went wrong and carry on, and a
    /// failed command is a normal event, not an exceptional one.
    pub fn run_line(&mut self, line: &str) -> Outcome {
        let command = match rsh_parser::parse(line) {
            Ok(Some(command)) => command,
            // A blank line is not a command and does not disturb `$?`. Typing
            // Enter at a prompt should never change what `$?` reports.
            Ok(None) => return Outcome::Continue,
            Err(error) => {
                report_parse_error(line, &error);
                self.last_status = EXIT_SYNTAX;
                return Outcome::Continue;
            }
        };

        match Builtin::lookup(command.program()) {
            Some(builtin) => {
                let (outcome, status) = builtin.run(&command, self.last_status);
                self.last_status = status;
                outcome
            }
            None => {
                self.last_status = self.run_external(&command);
                Outcome::Continue
            }
        }
    }

    /// Run a command as a child process, and wait for it.
    fn run_external(&mut self, command: &Command) -> i32 {
        let path =
            match rsh_process::resolve(command.program(), std::env::var_os("PATH").as_deref()) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("rsh: {error}");
                    return match error {
                        ResolveError::NotFound { .. } => EXIT_NOT_FOUND,
                        ResolveError::NotExecutable { .. } => EXIT_NOT_EXECUTABLE,
                    };
                }
            };

        let prepared = match rsh_process::Command::new(&path, command.argv()) {
            Ok(prepared) => prepared,
            Err(error) => {
                eprintln!("rsh: {error}");
                return EXIT_NOT_EXECUTABLE;
            }
        };

        // Flush before forking. The child inherits a copy of this buffer, and
        // anything still sitting in it would be written twice: once by us, once
        // by the child when it exits.
        let _ = std::io::stdout().flush();

        match prepared.spawn().and_then(rsh_process::Child::wait) {
            Ok(status) => status.code(),
            Err(error) => {
                eprintln!("rsh: {}: {error}", command.program());
                EXIT_NOT_EXECUTABLE
            }
        }
    }
}

/// Report a parse error, pointing at the character that caused it.
///
/// The caret is the whole reason `ParseError` carries a byte offset. "syntax
/// error near unexpected token" is what a shell says when it has thrown that
/// information away.
fn report_parse_error(line: &str, error: &rsh_parser::ParseError) {
    eprintln!("rsh: {error}");

    let offset = error.at().min(line.len());
    // Count characters, not bytes: a caret positioned by byte offset lands in
    // the wrong column the moment the line contains anything non-ASCII.
    let column = line[..offset].chars().count();
    eprintln!("  {}", line.trim_end());
    eprintln!("  {}^", " ".repeat(column));
}
