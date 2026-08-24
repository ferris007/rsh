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

use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::OnceLock;

use nix::errno::Errno;
use nix::libc::c_int;
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};

/// Set when `SIGINT` arrives — Ctrl-C.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// The signal that asked the shell to shut down, or 0.
static TERMINATING: AtomicI32 = AtomicI32::new(0);

/// Set when the terminal changes size.
static RESIZED: AtomicBool = AtomicBool::new(false);

/// The write end of the self-pipe, as a plain descriptor.
///
/// Read by handlers, so it cannot be behind a lock — a handler that blocked on
/// a mutex held by the interrupted thread would deadlock the process. An
/// `AtomicI32` set once at startup is the whole synchronisation.
static NOTIFY_WRITE: AtomicI32 = AtomicI32::new(-1);

/// The pipe itself, kept alive for the life of the process.
static NOTIFY: OnceLock<(OwnedFd, OwnedFd)> = OnceLock::new();

/// Wake anything waiting on the self-pipe.
///
/// # The problem this solves
///
/// A handler that only sets a flag is not enough for an event loop. The signal
/// can arrive *between* the check and the wait:
///
/// ```text
///     if flag.take() { ... }      ← flag is clear
///                                 ← signal arrives, handler sets flag
///     poller.wait(forever)        ← blocks, with the flag already set
/// ```
///
/// Nothing is left to wake the loop, and the event is lost until something
/// else happens to arrive. The window is small and it is real.
///
/// Writing a byte to a pipe closes it: a byte written before the wait begins is
/// still in the pipe when it starts, so the poller returns immediately. This is
/// the **self-pipe trick**, and it is what every portable event loop does with
/// signals. See `experiments/epoll`, which reproduces the race deterministically.
///
/// `write` is async-signal-safe. A full pipe means a wakeup is already pending,
/// so the failure is not merely tolerable — it means the job is already done.
fn notify() {
    let fd = NOTIFY_WRITE.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }

    // SAFETY: `write` is async-signal-safe. The descriptor is either -1, which
    // is checked above, or the pipe created at startup and never closed. The
    // buffer is a 'static byte string.
    unsafe { nix::libc::write(fd, b"!".as_ptr().cast(), 1) };
}

/// Set when a child changes state — exits, is stopped, or is continued.
///
/// Only useful once a job can outlive the command that started it. A shell that
/// waits for every child synchronously already knows when they end; one with
/// background jobs does not, and this is how it finds out without polling.
static CHILD_CHANGED: AtomicBool = AtomicBool::new(false);

/// Record a Ctrl-C.
///
/// Everything this does is a relaxed atomic store, which is a single
/// instruction and cannot block. The shell notices at the next place it can
/// safely act.
extern "C" fn on_interrupt(_signal: c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
    notify();
}

/// Record a request to shut down.
extern "C" fn on_terminate(signal: c_int) {
    TERMINATING.store(signal, Ordering::Relaxed);
    notify();
}

/// Record that the window changed size.
///
/// The new size is deliberately not read here. `ioctl` is not on the list of
/// functions a handler may call, and there is no hurry: the shell asks at the
/// next prompt, which is the only moment the answer is useful anyway.
extern "C" fn on_resize(_signal: c_int) {
    RESIZED.store(true, Ordering::Relaxed);
    notify();
}

/// Record that some child changed state.
///
/// Deliberately not `waitpid` — reaping here would race with the foreground
/// `waitpid` the shell is blocked in, and the handler would be reaping jobs the
/// main loop is in the middle of waiting for. The flag says "look when it is
/// safe"; the looking happens in the main loop.
extern "C" fn on_child(_signal: c_int) {
    CHILD_CHANGED.store(true, Ordering::Relaxed);
    notify();
}

/// Install the shell's handlers.
///
/// Deliberately *without* `SA_RESTART`. With it, a blocking read would resume
/// transparently after a signal and the shell would never learn that Ctrl-C
/// happened; without it, the read fails with `EINTR`, which is the shell's cue
/// to abandon the line and prompt again.
pub fn install() -> Result<(), Errno> {
    // The self-pipe first: a handler that fired before it existed would have
    // nowhere to write, and the wakeup would be lost.
    let (read, write) = nix::unistd::pipe()?;
    for end in [&read, &write] {
        // Close-on-exec, so children never inherit the shell's wakeup channel.
        nix::fcntl::fcntl(
            end,
            nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::FD_CLOEXEC),
        )?;
    }

    // Non-blocking, so a handler firing faster than the loop drains cannot
    // block inside a signal handler — which would be a deadlock with no way
    // out.
    nix::fcntl::fcntl(
        &write,
        nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
    )?;
    nix::fcntl::fcntl(
        &read,
        nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
    )?;

    NOTIFY_WRITE.store(write.as_raw_fd(), Ordering::Relaxed);
    let _ = NOTIFY.set((read, write));

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
    // SA_RESTART on SIGCHLD alone: a background job finishing is not a reason
    // to abandon the line the user is halfway through typing. Ctrl-C is; a
    // child exiting is not.
    let child = SigAction::new(
        SigHandler::Handler(on_child),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    // SA_RESTART for the same reason as SIGCHLD: someone dragging the window
    // is not a reason to abandon the line they are halfway through typing.
    let resize = SigAction::new(
        SigHandler::Handler(on_resize),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );

    for (signal, action) in [
        (Signal::SIGINT, &interrupt),
        (Signal::SIGTERM, &terminate),
        (Signal::SIGHUP, &terminate),
        // Ctrl-\, which would otherwise kill the shell and dump core.
        (Signal::SIGQUIT, &interrupt),
        (Signal::SIGCHLD, &child),
        (Signal::SIGWINCH, &resize),
    ] {
        // SAFETY: the handlers above do nothing but store to a `static`
        // atomic — no allocation, no locks, no calls into code that could do
        // either — so they are safe to run at an arbitrary interruption point.
        unsafe { sigaction(signal, action)? };
    }

    // Ignore the signals that would let the shell suspend itself.
    //
    // `tcsetpgrp` is a terminal write, so a shell that is not the foreground
    // group is signalled with SIGTTOU for calling it — which is exactly what
    // happens when it takes the terminal back from a job it just suspended.
    // The default action would stop the shell at the precise moment the user
    // pressed Ctrl-Z, leaving a frozen terminal and no shell to unfreeze it.
    //
    // SIGTSTP is ignored for the same reason: Ctrl-Z is meant for the
    // foreground job, and the shell is in that group until it hands the
    // terminal over.
    //
    // These are `SIG_IGN` rather than handlers because the shell has nothing to
    // do about them — and `SIG_IGN` is inherited across `exec`, so every child
    // resets them before running. See `Command::spawn`.
    let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
    for signal in [Signal::SIGTSTP, Signal::SIGTTIN, Signal::SIGTTOU] {
        // SAFETY: SIG_IGN installs no handler code.
        unsafe { sigaction(signal, &ignore)? };
    }

    Ok(())
}

/// The descriptor to watch for "a signal arrived".
///
/// Returns `None` before [`install`] has run. The caller registers this with a
/// poller and calls [`drain_notifications`] when it reports readable; *which*
/// signal arrived is then read from the flags below.
pub fn notify_fd() -> Option<RawFd> {
    NOTIFY.get().map(|(read, _)| read.as_raw_fd())
}

/// Throw away the bytes the handlers wrote.
///
/// The bytes carry no information — a handler cannot safely encode which signal
/// it was, and does not need to, because the flags already say. Their only job
/// is to make the pipe readable so the poller returns.
pub fn drain_notifications() {
    let Some(fd) = notify_fd() else {
        return;
    };

    let mut scratch = [0_u8; 64];
    loop {
        // SAFETY: reading into a stack buffer from a descriptor owned by the
        // static above, which is never closed.
        let count = unsafe { nix::libc::read(fd, scratch.as_mut_ptr().cast(), scratch.len()) };
        if count < scratch.len() as isize {
            break;
        }
    }
}

/// Whether the terminal has been resized since this was last called.
pub fn take_resize() -> bool {
    RESIZED.swap(false, Ordering::Relaxed)
}

/// Whether a child has changed state since this was last called.
pub fn take_child_event() -> bool {
    CHILD_CHANGED.swap(false, Ordering::Relaxed)
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

        // The flag alone is not enough for an event loop: a signal arriving
        // between the check and the wait leaves the loop blocked with nothing
        // to wake it. A byte in a pipe survives that gap.
        let fd = notify_fd().expect("the self-pipe should exist after install");
        drain_notifications();

        let mut scratch = [0_u8; 8];
        // SAFETY: reading into a stack buffer from the static self-pipe.
        let before = unsafe { nix::libc::read(fd, scratch.as_mut_ptr().cast(), scratch.len()) };
        assert!(before <= 0, "the pipe should be empty, read {before} bytes");

        raise(Signal::SIGINT).expect("failed to signal self");

        // SAFETY: as above.
        let after = unsafe { nix::libc::read(fd, scratch.as_mut_ptr().cast(), scratch.len()) };
        assert!(after > 0, "the handler should have written a wakeup byte");

        // The byte carries no meaning; the flag is where the answer is.
        assert!(take_interrupt());
    }
}
