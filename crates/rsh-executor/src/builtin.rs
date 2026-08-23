//! Commands the shell must run itself.
//!
//! A builtin is not "a command that happens to be fast". It is a command that
//! *cannot* be a child process, because its whole effect is a change to the
//! shell's own state. `/usr/bin/cd` could exist, and would do nothing useful:
//! it would change the working directory of a process that exits a moment
//! later, leaving the shell exactly where it was.
//!
//! That is the test for what belongs here. Phase 6 adds `jobs`, `fg`, and `bg`
//! for the same reason — the job table lives in this process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use rsh_parser::Command;

use crate::shell::Outcome;

/// Status returned by a builtin that was used incorrectly.
const EXIT_USAGE: i32 = 1;

/// Status for `exit` with a non-numeric argument, matching POSIX shells.
const EXIT_BAD_ARGUMENT: i32 = 2;

/// A command implemented by the shell itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Builtin {
    Cd,
    Exit,
}

impl Builtin {
    /// Match a command name against the builtin table.
    ///
    /// Builtins are checked before `PATH`, which is why `cd` works even on a
    /// system that also ships a `/usr/bin/cd`.
    pub(crate) fn lookup(name: &str) -> Option<Self> {
        match name {
            "cd" => Some(Self::Cd),
            "exit" => Some(Self::Exit),
            _ => None,
        }
    }

    /// Run the builtin, returning whether to continue and the new `$?`.
    pub(crate) fn run(self, command: &Command, last_status: i32) -> (Outcome, i32) {
        match self {
            Self::Cd => (Outcome::Continue, cd(command.args())),
            Self::Exit => exit(command.args(), last_status),
        }
    }
}

/// Change the working directory.
///
/// Handles the three forms a POSIX shell provides: no argument (go home), `-`
/// (go back), and an explicit path.
fn cd(args: &[String]) -> i32 {
    if args.len() > 1 {
        eprintln!("rsh: cd: too many arguments");
        return EXIT_USAGE;
    }

    // `cd -` prints where it landed. That is not decoration: the argument is
    // `$OLDPWD`, which the user cannot see, so without the echo they would not
    // know where they now are.
    let mut announce = false;

    let target: PathBuf = match args.first().map(String::as_str) {
        None | Some("~") => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home),
            None => {
                eprintln!("rsh: cd: HOME not set");
                return EXIT_USAGE;
            }
        },
        Some("-") => match std::env::var_os("OLDPWD") {
            Some(old) => {
                announce = true;
                PathBuf::from(old)
            }
            None => {
                eprintln!("rsh: cd: OLDPWD not set");
                return EXIT_USAGE;
            }
        },
        Some(path) => PathBuf::from(path),
    };

    // Read the old directory *before* moving, so OLDPWD is accurate even if
    // the change fails halfway through a symlinked path.
    let previous = std::env::current_dir().ok();

    if let Err(error) = std::env::set_current_dir(&target) {
        eprintln!("rsh: cd: {}: {error}", target.display());
        return EXIT_USAGE;
    }

    update_pwd(previous.as_deref());

    if announce {
        if let Ok(current) = std::env::current_dir() {
            println!("{}", current.display());
        }
    }

    0
}

/// Keep `PWD` and `OLDPWD` in step with the process's real directory.
///
/// These are environment variables, not shell state, because child processes
/// read them. A program that prints `$PWD` and a program that calls `getcwd()`
/// should not disagree, so `PWD` is set from `current_dir()` — the kernel's
/// answer — rather than from the path the user typed, which may be relative or
/// contain `..`.
fn update_pwd(previous: Option<&Path>) {
    if let Some(previous) = previous {
        std::env::set_var("OLDPWD", OsString::from(previous));
    }
    if let Ok(current) = std::env::current_dir() {
        std::env::set_var("PWD", OsString::from(current));
    }
}

/// Leave the shell.
///
/// With no argument, exits with the status of the last command — so
/// `some-command; exit` propagates that command's result, which is what makes
/// `exit` usable as the last line of a script.
fn exit(args: &[String], last_status: i32) -> (Outcome, i32) {
    match args {
        [] => (Outcome::Exit(last_status), last_status),
        [code] => match code.parse::<i32>() {
            Ok(code) => (Outcome::Exit(code), code),
            Err(_) => {
                // POSIX shells exit anyway, with status 2. Refusing to exit
                // would be worse: `exit` is how a user gets out, and a typo
                // should not trap them in the shell.
                eprintln!("rsh: exit: {code}: numeric argument required");
                (Outcome::Exit(EXIT_BAD_ARGUMENT), EXIT_BAD_ARGUMENT)
            }
        },
        _ => {
            // Here the shell does *not* exit: with several arguments it cannot
            // tell which status was meant, and guessing would be worse than
            // asking again.
            eprintln!("rsh: exit: too many arguments");
            (Outcome::Continue, EXIT_USAGE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_recognised_by_exact_name() {
        assert_eq!(Builtin::lookup("cd"), Some(Builtin::Cd));
        assert_eq!(Builtin::lookup("exit"), Some(Builtin::Exit));
        assert_eq!(Builtin::lookup("echo"), None);
        assert_eq!(Builtin::lookup("CD"), None);
        assert_eq!(Builtin::lookup(""), None);
    }

    #[test]
    fn exit_without_an_argument_carries_the_last_status() {
        assert_eq!(exit(&[], 7), (Outcome::Exit(7), 7));
        assert_eq!(exit(&[], 0), (Outcome::Exit(0), 0));
    }

    #[test]
    fn exit_takes_an_explicit_status() {
        assert_eq!(exit(&["3".to_owned()], 0), (Outcome::Exit(3), 3));
    }

    #[test]
    fn exit_with_a_non_numeric_argument_still_exits() {
        assert_eq!(exit(&["banana".to_owned()], 0), (Outcome::Exit(2), 2));
    }

    #[test]
    fn exit_with_several_arguments_does_not_exit() {
        let args = ["1".to_owned(), "2".to_owned()];
        assert_eq!(exit(&args, 0), (Outcome::Continue, EXIT_USAGE));
    }

    // `cd` is exercised end-to-end in the integration tests instead: it mutates
    // process-wide state (the working directory and the environment), which
    // makes it unsafe to assert against from a multi-threaded test harness
    // where another test may be reading that same state.
}
