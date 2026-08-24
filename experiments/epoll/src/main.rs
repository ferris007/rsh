//! Experiment: why is a flag set by a signal handler not enough?
//!
//! See `README.md` in this directory for the question and the observation.
//!
//! The race is normally a matter of microseconds and nearly impossible to
//! observe on purpose. This program makes it deterministic by *blocking* the
//! signal, arranging for it to be pending, and then unblocking it at a moment
//! of its choosing — so the handler is guaranteed to run in the gap between the
//! flag being checked and the wait beginning.

use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

use nix::libc;
use nix::sys::signal::{raise, sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use nix::unistd::pipe;
use whelk_event::{Poller, Token};

/// How long the loop is willing to wait for something to happen.
///
/// Long enough that "returned immediately" and "waited the whole time" are not
/// in any doubt.
const PATIENCE: Duration = Duration::from_millis(800);

/// Set by the handler, as a shell's handlers do.
static ARRIVED: AtomicBool = AtomicBool::new(false);

/// The write end of the self-pipe, or -1 when the experiment is not using one.
static NOTIFY: AtomicI32 = AtomicI32::new(-1);

/// The signal handler.
///
/// Sets a flag always; writes a byte only when a self-pipe is configured. That
/// single difference is the whole experiment.
extern "C" fn on_signal(_signal: libc::c_int) {
    ARRIVED.store(true, Ordering::Relaxed);

    let fd = NOTIFY.load(Ordering::Relaxed);
    if fd >= 0 {
        // SAFETY: `write` is async-signal-safe; the descriptor is the pipe
        // created below and is open for the life of the run.
        unsafe { libc::write(fd, b"!".as_ptr().cast(), 1) };
    }
}

fn main() {
    install();

    println!("a signal arrives between checking the flag and starting the wait");
    println!(
        "the loop then waits up to {}ms for something to happen",
        PATIENCE.as_millis()
    );
    println!();

    let (read, write) = pipe().expect("failed to create a pipe");

    println!("with a flag alone:");
    NOTIFY.store(-1, Ordering::Relaxed);
    report(run(&read));

    println!("with a self-pipe as well:");
    NOTIFY.store(write.as_raw_fd(), Ordering::Relaxed);
    report(run(&read));

    drop(write);
}

/// Install a handler that does what a shell's does.
fn install() {
    let action = SigAction::new(
        SigHandler::Handler(on_signal),
        SaFlags::empty(),
        SigSet::empty(),
    );

    // SAFETY: the handler stores to statics and, at most, calls `write`. Both
    // are safe at an arbitrary interruption point.
    unsafe { sigaction(Signal::SIGUSR1, &action) }.expect("failed to install a handler");
}

/// Run one round, returning how long the wait took and whether it saw anything.
fn run(read: &OwnedFd) -> (Duration, bool, bool) {
    ARRIVED.store(false, Ordering::Relaxed);
    drain(read);

    let mut poller = Poller::new().expect("failed to create a poller");
    poller
        .watch(read.as_raw_fd(), Token(1))
        .expect("failed to watch the pipe");

    // Block the signal, make it pending, then let it through at a moment of our
    // choosing. Without this the race is a few microseconds wide and shows up
    // once in a great many runs.
    let mut only_usr1 = SigSet::empty();
    only_usr1.add(Signal::SIGUSR1);
    only_usr1.thread_block().expect("failed to block");

    raise(Signal::SIGUSR1).expect("failed to raise");

    // The loop checks its flag. Nothing yet — the signal is still pending.
    let seen_before_wait = ARRIVED.load(Ordering::Relaxed);

    // ...and here the signal is delivered, in the gap. The handler runs now.
    only_usr1.thread_unblock().expect("failed to unblock");

    // The loop now waits, having already decided there was nothing to do.
    let started = Instant::now();
    let woke = !poller.wait(Some(PATIENCE)).expect("wait failed").is_empty();
    let elapsed = started.elapsed();

    (elapsed, seen_before_wait, woke)
}

/// Empty the pipe between rounds.
fn drain(read: &OwnedFd) {
    let mut scratch = [0_u8; 64];
    loop {
        // SAFETY: a non-blocking read into a stack buffer. The pipe is
        // blocking, so this is only called when something is known to be in it
        // — or not at all, which is why the length check ends the loop.
        let mut poller = Poller::new().expect("failed to create a poller");
        poller
            .watch(read.as_raw_fd(), Token(1))
            .expect("failed to watch");
        if poller
            .wait(Some(Duration::ZERO))
            .expect("wait failed")
            .is_empty()
        {
            return;
        }

        // SAFETY: as above.
        let count =
            unsafe { libc::read(read.as_raw_fd(), scratch.as_mut_ptr().cast(), scratch.len()) };
        if count <= 0 {
            return;
        }
    }
}

fn report((elapsed, seen_before_wait, woke): (Duration, bool, bool)) {
    println!("  flag before the wait: {seen_before_wait}");
    println!("  the wait returned after {}ms", elapsed.as_millis());
    if woke {
        println!("  woken by the pipe — the signal was not missed");
    } else {
        println!("  nothing arrived: the loop slept through a signal it had already handled");
    }
    println!();
}
