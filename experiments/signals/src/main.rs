//! Experiment: what happens to a pipeline when the shell receives SIGINT?
//!
//! See `README.md` in this directory for the question and the observation.
//!
//! The program starts two children — one left in its parent's process group,
//! one moved into its own — then signals the group and reports which of them
//! survived. That single difference is the whole of job control.

use std::ffi::CString;
use std::time::Duration;

use nix::libc;
use nix::sys::signal::{killpg, SigHandler, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{fork, getpgrp, setpgid, ForkResult, Pid};

/// Long enough for a child to finish `exec` before the signal arrives.
const SETTLE: Duration = Duration::from_millis(300);

fn main() {
    // Move into a process group of our own before doing anything else.
    //
    // Without this the group we signal below is the one we were launched in —
    // which contains the shell, or `cargo test`, or whatever else started this
    // program. Running the experiment would interrupt the thing running it.
    // That is not a quirk of the experiment; it is the reason a shell puts
    // every job in its own group.
    setpgid(Pid::from_raw(0), Pid::from_raw(0)).expect("failed to create a process group");

    let group = getpgrp();
    println!("this process group: {group} (moved here, so nothing outside is signalled)");
    println!();

    let same_group = spawn_sleeper(false);
    let own_group = spawn_sleeper(true);
    println!("child in the same group: {same_group}");
    println!("child in its own group:  {own_group}");
    println!();

    std::thread::sleep(SETTLE);

    // Ignore it for ourselves *after* forking, so the children inherited the
    // default action rather than SIG_IGN. A shell does the same thing with a
    // handler, which `exec` resets for free.
    //
    // SAFETY: SIG_IGN installs no code, so there is no handler to be unsound.
    unsafe { nix::sys::signal::signal(Signal::SIGINT, SigHandler::SigIgn) }
        .expect("failed to ignore SIGINT");

    println!("sending SIGINT to process group {group}");
    killpg(group, Signal::SIGINT).expect("failed to signal the group");

    std::thread::sleep(SETTLE);

    println!("  same group: {}", describe(same_group));
    println!("  own group:  {}", describe(own_group));

    // Leave nothing behind.
    let _ = nix::sys::signal::kill(own_group, Signal::SIGKILL);
    let _ = waitpid(own_group, None);
}

/// Fork a child that sleeps, optionally in a process group of its own.
fn spawn_sleeper(new_group: bool) -> Pid {
    let sleep = CString::new("/bin/sleep").expect("no NUL in a literal");
    let duration = CString::new("30").expect("no NUL in a literal");
    let argv: Vec<*const libc::c_char> = vec![sleep.as_ptr(), duration.as_ptr(), std::ptr::null()];

    // SAFETY: the child calls only async-signal-safe functions — `setpgid`,
    // `signal`, `execv`, `_exit` — on pointers built before the fork.
    match unsafe { fork() }.expect("fork failed") {
        ForkResult::Child => {
            if new_group {
                // The child does this itself rather than waiting for the
                // parent, because either might run first and the child must be
                // in its group before it can be signalled. A real shell does it
                // in *both* places for exactly that reason.
                let _ = setpgid(Pid::from_raw(0), Pid::from_raw(0));
            }

            // SAFETY: restoring the default action, which installs no code.
            let _ = unsafe { nix::sys::signal::signal(Signal::SIGINT, SigHandler::SigDfl) };

            // SAFETY: `argv` is NULL-terminated and alive in this address-space
            // copy. On success this does not return.
            unsafe { libc::execv(sleep.as_ptr(), argv.as_ptr()) };

            // SAFETY: `_exit` terminates without flushing inherited buffers.
            unsafe { libc::_exit(127) }
        }
        ForkResult::Parent { child } => child,
    }
}

/// Report whether a child died of the signal or is still running.
fn describe(pid: Pid) -> &'static str {
    match waitpid(pid, Some(WaitPidFlag::WNOHANG)) {
        Ok(WaitStatus::Signaled(_, Signal::SIGINT, _)) => "killed by SIGINT",
        Ok(WaitStatus::StillAlive) => "still running — the signal never reached it",
        Ok(other) => {
            println!("    (unexpected: {other:?})");
            "unexpected"
        }
        Err(error) => {
            println!("    (waitpid failed: {error})");
            "unknown"
        }
    }
}
