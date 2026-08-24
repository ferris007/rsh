//! Who owns the terminal — which process group may use it.
//!
//! A terminal has exactly one **foreground process group**. That single piece
//! of kernel state decides three things at once:
//!
//! * which processes receive `SIGINT` when Ctrl-C is typed;
//! * which processes may read from the terminal — anyone else gets `SIGTTIN`
//!   and is stopped;
//! * which processes may write to it, if `TOSTOP` is set.
//!
//! Running a job in the foreground *is* giving it that slot. Suspending it is
//! taking the slot back. Job control is very nearly this one variable.

use std::os::fd::BorrowedFd;

use nix::errno::Errno;
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::unistd::{isatty, tcgetpgrp, tcsetpgrp, Pid};

/// The descriptor the shell uses to talk to its terminal.
///
/// Standard input, because that is the one a user's terminal is reliably
/// attached to. `rsh > log` redirects stdout and stays interactive; nothing a
/// user does redirects stdin and expects a prompt.
pub(crate) fn terminal() -> BorrowedFd<'static> {
    // SAFETY: descriptor 0 is standard input, open for the life of the process.
    // The borrow does not escape the call it is passed to.
    unsafe { BorrowedFd::borrow_raw(0) }
}

/// Whether the shell has a terminal to control.
///
/// When it does not — a script, a pipe, a CI runner — job control is switched
/// off entirely rather than half-performed. There is no terminal to hand over,
/// nobody to type Ctrl-Z, and no reason to put jobs in groups of their own.
pub fn is_interactive() -> bool {
    isatty(terminal()).unwrap_or(false)
}

/// The process group currently owning the terminal.
pub fn foreground_group() -> Option<Pid> {
    tcgetpgrp(terminal()).ok()
}

/// Give the terminal to a process group.
///
/// # The `SIGTTOU` problem
///
/// `tcsetpgrp` is itself a terminal operation, so a process that is *not* in
/// the foreground group is signalled with `SIGTTOU` for calling it — and the
/// default action for `SIGTTOU` is to stop the process.
///
/// Which is exactly the situation a shell is in when it takes the terminal
/// back from a job it just suspended. A shell that did not handle this would
/// stop itself at the precise moment the user pressed Ctrl-Z, leaving a frozen
/// terminal and no shell to unfreeze it.
///
/// The fix is to ignore `SIGTTOU` across the call. `rsh` ignores it for the
/// whole session, and resets it in every child so that the shell's
/// self-protection does not become a property of every program it runs. This
/// function ignores it again locally, so the operation stays correct even if
/// someone changes that policy later.
pub fn give_to(pgid: Pid) -> Result<(), Errno> {
    let previous = ignore_ttou()?;
    let result = tcsetpgrp(terminal(), pgid);
    restore(previous)?;
    result
}

/// Ignore `SIGTTOU`, returning the previous disposition.
fn ignore_ttou() -> Result<SigAction, Errno> {
    let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
    // SAFETY: SIG_IGN installs no handler code, so there is nothing that could
    // run unsoundly in a signal context.
    unsafe { sigaction(Signal::SIGTTOU, &ignore) }
}

/// Put a signal disposition back.
fn restore(previous: SigAction) -> Result<(), Errno> {
    // SAFETY: restoring a disposition captured from this process moments ago.
    unsafe { sigaction(Signal::SIGTTOU, &previous) }?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_harness_has_no_controlling_terminal() {
        // Not an assertion about the shell so much as about the environment:
        // these tests run without a terminal, which is why the job-control
        // tests drive `rsh` through a pipe and check that it *declines* to do
        // job control rather than doing it badly.
        if is_interactive() {
            assert!(
                foreground_group().is_some(),
                "a terminal must have a foreground group"
            );
        } else {
            assert_eq!(foreground_group(), None);
        }
    }

    #[test]
    fn the_descriptor_answers_consistently() {
        // `isatty` and `tcgetpgrp` must agree; a terminal always has a
        // foreground group, and a pipe never does.
        assert_eq!(is_interactive(), foreground_group().is_some());
    }
}
