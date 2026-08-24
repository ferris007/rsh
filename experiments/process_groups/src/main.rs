//! Experiment: what happens when a background job reads from the terminal?
//!
//! See `README.md` in this directory for the question and the observation.
//!
//! # Why this is more elaborate than it looks
//!
//! `SIGTTIN` is only sent for a read from a process's **controlling terminal**,
//! and a process only has one if it is in a session that acquired it. So the
//! demonstration cannot just open a pty and read from it — it has to run inside
//! a session that owns one.
//!
//! `forkpty` does exactly that: the child becomes a session leader with the new
//! pty as its controlling terminal. It plays the part of the shell, and the
//! parent reads the master side and relays what it saw.

use std::ffi::CString;
use std::io::Read;
use std::time::Duration;

use nix::libc;
use nix::pty::{forkpty, ForkptyResult};
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{fork, pipe, setpgid, tcsetpgrp, write, ForkResult, Pid};
use std::os::fd::AsRawFd;

/// Long enough for a child to reach its `read` before we look.
const SETTLE: Duration = Duration::from_millis(400);

fn main() {
    // SAFETY: `forkpty` forks. The child branch runs the demonstration below
    // and always terminates through `_exit`, never returning into `main`.
    let result = unsafe { forkpty(None, None) }.expect("failed to open a pseudoterminal");

    match result {
        ForkptyResult::Child => {
            demonstrate();
            // SAFETY: `_exit` terminates without flushing inherited buffers.
            unsafe { libc::_exit(0) }
        }
        ForkptyResult::Parent { child, master } => {
            let mut transcript = String::new();
            let mut reader = std::fs::File::from(master);
            let _ = reader.read_to_string(&mut transcript);
            let _ = waitpid(child, None);

            // The pty echoes and translates newlines; strip the carriage
            // returns so the output reads the same as any other program's.
            print!("{}", transcript.replace('\r', ""));
        }
    }
}

/// The part that runs inside a session owning a terminal.
fn demonstrate() {
    // A gate, so the children do not touch the terminal before the parent has
    // finished arranging who owns it.
    //
    // Without this the demonstration is a race, and one this program loses on a
    // busy machine: a child that reaches `read` before `tcsetpgrp` runs is *not*
    // in the foreground group yet, gets SIGTTIN, and stops — even the one about
    // to be given the terminal. Both children then report as stopped and the
    // experiment appears to show something it does not.
    //
    // A real shell has the same problem and solves it the same way. This is why
    // `fg` hands over the terminal before sending SIGCONT, rather than after.
    let (gate_read, gate_write) = pipe().expect("failed to create a pipe");

    // Two children, each leading its own process group — exactly as a shell's
    // jobs do. Both will try to read from the terminal, once let through.
    let foreground = spawn_reader(&gate_read);
    let background = spawn_reader(&gate_read);

    println!("two children, each in its own process group:");
    println!("  {foreground} and {background}");
    println!();

    // A terminal has exactly one foreground process group, so this is a
    // choice, not a setting.
    match tcsetpgrp(std::io::stdin(), foreground) {
        Ok(()) => println!("gave the terminal to {foreground}"),
        Err(error) => println!("could not hand over the terminal: {error}"),
    }
    println!();

    // Now, and only now, let them read.
    write(&gate_write, b"gg").expect("failed to open the gate");
    drop(gate_write);

    std::thread::sleep(SETTLE);

    println!("  foreground: {}", describe(foreground));
    println!("  background: {}", describe(background));

    for pid in [foreground, background] {
        let _ = kill(pid, Signal::SIGKILL);
        let _ = waitpid(pid, None);
    }
}

/// Fork a child that leads its own group and, once let through, reads the
/// terminal.
fn spawn_reader(gate: &std::os::fd::OwnedFd) -> Pid {
    let cat = CString::new("/bin/cat").expect("no NUL in a literal");
    let argv: Vec<*const libc::c_char> = vec![cat.as_ptr(), std::ptr::null()];

    // SAFETY: the child calls only async-signal-safe functions — `setpgid`,
    // `execv`, `_exit` — on data prepared before the fork.
    match unsafe { fork() }.expect("fork failed") {
        ForkResult::Child => {
            let _ = setpgid(Pid::from_raw(0), Pid::from_raw(0));

            // Wait at the gate. Reading a pipe is not reading the terminal, so
            // this cannot itself earn a SIGTTIN.
            //
            // SAFETY: `read` is async-signal-safe; the buffer is on this
            // process's stack and the descriptor was inherited across the fork.
            let mut byte = [0_u8; 1];
            unsafe { libc::read(gate.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };

            // SAFETY: `argv` is NULL-terminated and alive in this address-space
            // copy. On success this does not return.
            unsafe { libc::execv(cat.as_ptr(), argv.as_ptr()) };

            // SAFETY: `_exit` terminates without flushing inherited buffers.
            unsafe { libc::_exit(127) }
        }
        ForkResult::Parent { child } => {
            // Set from the parent too: either side may run first after the
            // fork, and the group must exist before the terminal can be handed
            // to it. One of the two calls is always redundant, and which one is
            // not knowable in advance.
            let _ = setpgid(child, child);
            child
        }
    }
}

/// Report whether a child is running or has been stopped.
fn describe(pid: Pid) -> String {
    match waitpid(pid, Some(WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED)) {
        Ok(WaitStatus::Stopped(_, Signal::SIGTTIN)) => {
            "stopped by SIGTTIN — it may not read".to_owned()
        }
        Ok(WaitStatus::Stopped(_, signal)) => format!("stopped by {signal}"),
        Ok(WaitStatus::StillAlive) => "running, blocked in read — it owns the terminal".to_owned(),
        Ok(other) => format!("unexpected: {other:?}"),
        Err(error) => format!("waitpid failed: {error}"),
    }
}
