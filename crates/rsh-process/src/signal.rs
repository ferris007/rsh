//! Signal handling.
//!
//! # The constraint
//!
//! A signal handler runs by interrupting whatever the process was doing —
//! possibly mid-`malloc`, holding the allocator's lock. If the handler then
//! allocates, it deadlocks against a lock its own thread holds. POSIX resolves
//! this by permitting only [async-signal-safe] functions inside a handler, the
//! same rule that governs the window after `fork`.
//!
//! In Rust that means: no `println!`, no formatting, no allocation, no locks,
//! nothing that could allocate three calls deep. Which rules out almost
//! everything.
//!
//! # What is left
//!
//! Setting a flag. The handlers here store into an atomic and return; the shell
//! reads those flags at points where it is safe to do real work. This is the
//! standard shape — a handler records that something happened, and the main
//! loop decides what it means.
//!
//! Atomics are usable here because a relaxed store to an `AtomicBool` compiles
//! to a plain instruction with no lock and no call. Nothing else in the
//! handlers can block.
//!
//! # Why handlers, not `SIG_IGN`
//!
//! The shell could simply ignore `SIGINT` so that Ctrl-C does not kill it. It
//! installs a handler instead, and the reason is inheritance:
//!
//! * `exec` **resets handlers** to the default action — the new program does
//!   not contain the old one's code, so a function pointer would be nonsense.
//! * `exec` **keeps `SIG_IGN`**, because "ignore" needs no code.
//!
//! A shell that ignored `SIGINT` would hand every child a program that cannot
//! be interrupted. With a handler, each child starts with the default action
//! and no extra work. See `experiments/pipes`, which is the same lesson learned
//! from `SIGPIPE`.
//!
//! [async-signal-safe]: https://man7.org/linux/man-pages/man7/signal-safety.7.html

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use nix::errno::Errno;
use nix::libc::c_int;
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};

/// Set when `SIGINT` arrives — Ctrl-C.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// The signal that asked the shell to shut down, or 0.
static TERMINATING: AtomicI32 = AtomicI32::new(0);

/// Record a Ctrl-C.
///
/// Everything this does is a relaxed atomic store, which is a single
/// instruction and cannot block. The shell notices at the next place it can
/// safely act.
extern "C" fn on_interrupt(_signal: c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Record a request to shut down.
extern "C" fn on_terminate(signal: c_int) {
    TERMINATING.store(signal, Ordering::Relaxed);
}

/// Install the shell's handlers.
///
/// Deliberately *without* `SA_RESTART`. With it, a blocking read would resume
/// transparently after a signal and the shell would never learn that Ctrl-C
/// happened; without it, the read fails with `EINTR`, which is the shell's cue
/// to abandon the line and prompt again.
pub fn install() -> Result<(), Errno> {
    let interrupt = SigAction::new(
        SigHandler::Handler(on_interrupt),
        SaFlags::empty(),
        SigSet::empty(),
    );
    let terminate = SigAction::new(
        SigHandler::Handler(on_terminate),
        SaFlags::empty(),
        SigSet::empty(),
    );

    for (signal, action) in [
        (Signal::SIGINT, &interrupt),
        (Signal::SIGTERM, &terminate),
        (Signal::SIGHUP, &terminate),
        // Ctrl-\, which would otherwise kill the shell and dump core.
        (Signal::SIGQUIT, &interrupt),
    ] {
        // SAFETY: the handlers above do nothing but store to a `static`
        // atomic — no allocation, no locks, no calls into code that could do
        // either — so they are safe to run at an arbitrary interruption point.
        unsafe { sigaction(signal, action)? };
    }

    Ok(())
}

/// Whether a Ctrl-C has arrived since this was last called, clearing the flag.
///
/// Taking rather than peeking means a signal is acted on once. The alternative
/// leaves a stale flag that fires again at the next prompt, which looks to a
/// user like a second Ctrl-C they did not type.
pub fn take_interrupt() -> bool {
    INTERRUPTED.swap(false, Ordering::Relaxed)
}

/// The signal that asked the shell to shut down, if one has.
///
/// Not cleared: a shutdown request does not expire, and a shell that forgot one
/// because it checked twice would keep running after being told to stop.
pub fn shutdown_requested() -> Option<Signal> {
    match TERMINATING.load(Ordering::Relaxed) {
        0 => None,
        raw => Signal::try_from(raw).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::raise;

    // These run in-process and mutate global signal state, so they are one
    // test: a `#[test]` per signal would race with its neighbours in the
    // multi-threaded harness.
    #[test]
    fn handlers_record_signals_without_killing_the_process() {
        install().expect("failed to install handlers");

        assert!(!take_interrupt(), "no interrupt should be pending yet");

        // `raise` rather than `kill`: it delivers to the calling thread and the
        // handler has run by the time it returns. `kill` targets the process,
        // which in a multi-threaded harness means an arbitrary thread and no
        // ordering guarantee against the assertion below.
        raise(Signal::SIGINT).expect("failed to signal self");
        assert!(take_interrupt(), "SIGINT was not recorded");

        // Taken, not peeked: the second read must come back empty.
        assert!(!take_interrupt(), "the flag was not cleared");

        // Reaching this line at all is the other half of the assertion — the
        // default action for SIGINT would have terminated the test runner.
        assert_eq!(shutdown_requested(), None);
    }
}
