//! What happened to a child, and what number the user sees.

use std::fmt;

use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;

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

/// What a child did, as reported by a non-blocking check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildEvent {
    /// It ended.
    Finished(Pid, ExitStatus),
    /// It was suspended.
    Stopped(Pid, Signal),
    /// It was resumed.
    Continued(Pid),
}

impl ChildEvent {
    /// Which process the event is about.
    pub fn pid(self) -> Pid {
        match self {
            Self::Finished(pid, _) | Self::Stopped(pid, _) | Self::Continued(pid) => pid,
        }
    }
}

/// Collect every child state change that is waiting, without blocking.
///
/// `WNOHANG` is what makes this safe to call from the main loop: it reports
/// what has already happened and returns immediately if nothing has.
/// `WUNTRACED` and `WCONTINUED` widen "happened" from "died" to "changed
/// state", which is the difference between a shell that can only notice
/// completed jobs and one that can track suspended ones.
///
/// Called at the prompt rather than in the `SIGCHLD` handler. Reaping inside
/// the handler would race with whatever `waitpid` the shell is already blocked
/// in for a foreground job — the handler would collect a status the main loop
/// is waiting for, and the main loop would then wait forever for a child that
/// no longer exists.
pub fn collect_child_events() -> Vec<ChildEvent> {
    let flags = WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED | WaitPidFlag::WCONTINUED;
    let mut events = Vec::new();

    loop {
        // `Pid::from_raw(-1)` means "any child", which is the only form that
        // works here: the shell does not know which job the news is about.
        match waitpid(Pid::from_raw(-1), Some(flags)) {
            Ok(WaitStatus::StillAlive) => break,
            Ok(WaitStatus::Exited(pid, code)) => {
                events.push(ChildEvent::Finished(pid, ExitStatus::Exited(code)));
            }
            Ok(WaitStatus::Signaled(pid, signal, _)) => {
                events.push(ChildEvent::Finished(pid, ExitStatus::Signaled(signal)));
            }
            Ok(WaitStatus::Stopped(pid, signal)) => events.push(ChildEvent::Stopped(pid, signal)),
            Ok(WaitStatus::Continued(pid)) => events.push(ChildEvent::Continued(pid)),
            Ok(_) => continue,
            // No children at all, which is the ordinary state of an idle shell.
            Err(Errno::ECHILD) => break,
            Err(Errno::EINTR) => continue,
            Err(_) => break,
        }
    }

    events
}

/// Wait for the next child state change, blocking until one arrives.
///
/// The blocking counterpart of [`collect_child_events`], for a shell that has
/// nothing to do until a specific job moves — `fg`, which has handed over the
/// terminal and is waiting to get it back.
///
/// Returns an empty vector if there are no children at all, so a caller looping
/// on this cannot spin forever waiting for news that can never come.
pub fn collect_child_events_blocking() -> Vec<ChildEvent> {
    let flags = WaitPidFlag::WUNTRACED | WaitPidFlag::WCONTINUED;

    loop {
        match waitpid(Pid::from_raw(-1), Some(flags)) {
            Ok(WaitStatus::Exited(pid, code)) => {
                return vec![ChildEvent::Finished(pid, ExitStatus::Exited(code))]
            }
            Ok(WaitStatus::Signaled(pid, signal, _)) => {
                return vec![ChildEvent::Finished(pid, ExitStatus::Signaled(signal))]
            }
            Ok(WaitStatus::Stopped(pid, signal)) => return vec![ChildEvent::Stopped(pid, signal)],
            Ok(WaitStatus::Continued(pid)) => return vec![ChildEvent::Continued(pid)],
            Ok(_) => continue,
            Err(Errno::ECHILD) => return Vec::new(),
            Err(Errno::EINTR) => continue,
            Err(_) => return Vec::new(),
        }
    }
}
