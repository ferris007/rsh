//! What happened to a child, and what number the user sees.

use std::fmt;

use nix::sys::signal::Signal;
use nix::sys::wait::WaitStatus;

/// Reported when a command could not be found on `PATH`.
///
/// POSIX-conventional, and load-bearing: build systems and `command -v`
/// wrappers branch on exactly this value.
pub const EXIT_NOT_FOUND: i32 = 127;

/// Reported when a command was found but could not be executed.
pub const EXIT_NOT_EXECUTABLE: i32 = 126;

/// Base for statuses that report death by signal: the shell reports `128 + n`.
pub const EXIT_SIGNAL_BASE: i32 = 128;

/// How a child process ended.
///
/// `waitpid` hands back an encoded `int` whose meaning depends on which
/// `WIFEXITED`-family macro you ask. Decoding it once, here, keeps that
/// encoding from leaking into the rest of the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    /// The process called `exit(code)` or returned from `main`.
    Exited(i32),
    /// The process was killed by a signal.
    Signaled(Signal),
}

impl ExitStatus {
    /// The status the shell reports as `$?`.
    ///
    /// Death by signal becomes `128 + signal`. The convention exists because
    /// `$?` is a single byte in wait status terms and has to encode both
    /// outcomes; every POSIX shell reports it this way, so scripts that check
    /// for 130 (`128 + SIGINT`) work against `rsh` too.
    pub fn code(self) -> i32 {
        match self {
            Self::Exited(code) => code,
            Self::Signaled(sig) => EXIT_SIGNAL_BASE + sig as i32,
        }
    }

    /// Whether the command reported success.
    pub fn success(self) -> bool {
        matches!(self, Self::Exited(0))
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited(code) => write!(f, "exited with {code}"),
            Self::Signaled(sig) => write!(f, "killed by {sig}"),
        }
    }
}

impl ExitStatus {
    /// Decode a `waitpid` result.
    ///
    /// Returns `None` for statuses that are not a termination — `Stopped` and
    /// `Continued` are job-control events (Phase 6), and treating them as an
    /// exit here would silently reap a suspended job.
    pub(crate) fn from_wait(status: WaitStatus) -> Option<Self> {
        match status {
            WaitStatus::Exited(_, code) => Some(Self::Exited(code)),
            WaitStatus::Signaled(_, sig, _) => Some(Self::Signaled(sig)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::unistd::Pid;

    #[test]
    fn exit_codes_pass_through() {
        assert_eq!(ExitStatus::Exited(0).code(), 0);
        assert_eq!(ExitStatus::Exited(42).code(), 42);
        assert!(ExitStatus::Exited(0).success());
        assert!(!ExitStatus::Exited(1).success());
    }

    #[test]
    fn signals_report_as_128_plus_signal() {
        // 130 is what a script sees when a command is interrupted with Ctrl-C.
        assert_eq!(ExitStatus::Signaled(Signal::SIGINT).code(), 130);
        assert_eq!(ExitStatus::Signaled(Signal::SIGKILL).code(), 137);
        assert!(!ExitStatus::Signaled(Signal::SIGINT).success());
    }

    #[test]
    fn stop_and_continue_are_not_terminations() {
        let pid = Pid::from_raw(1234);
        assert_eq!(
            ExitStatus::from_wait(WaitStatus::Exited(pid, 3)),
            Some(ExitStatus::Exited(3))
        );
        assert_eq!(
            ExitStatus::from_wait(WaitStatus::Stopped(pid, Signal::SIGTSTP)),
            None
        );
    }
}
