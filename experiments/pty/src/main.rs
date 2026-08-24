//! Experiment: why does a program's output change when you pipe it?
//!
//! See `README.md` in this directory for the question and the observation.
//!
//! The same program is run twice — once with its output on a pipe, once on a
//! pseudoterminal — and the difference in *when* its bytes arrive is the whole
//! answer.

use std::ffi::CString;
use std::io::Read;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

use nix::libc;
use nix::pty::openpty;
use nix::sys::wait::waitpid;
use nix::unistd::{dup2_stderr, dup2_stdout, fork, pipe, ForkResult};

/// The program under observation: writes a line, pauses, writes another.
///
/// The pause is the point. If its output is line-buffered, the first line
/// arrives during the pause; if it is fully buffered, nothing arrives until the
/// program exits and the C library flushes.
fn writer_path() -> std::path::PathBuf {
    std::env::current_exe()
        .expect("cannot find this program")
        .parent()
        .expect("a binary has a directory")
        .join("xp-pty-writer")
}

/// How long to wait before checking what has arrived.
const PEEK_AFTER: Duration = Duration::from_millis(400);

fn main() {
    println!("running: xp-pty-writer, which prints a line, sleeps, prints another");
    println!(
        "checking what has arrived after {}ms",
        PEEK_AFTER.as_millis()
    );
    println!();

    let (pipe_read, pipe_write) = pipe().expect("failed to create a pipe");
    println!("with output on a pipe:");
    report(run_with_output(pipe_write, pipe_read));

    let pty = openpty(None, None).expect("failed to open a pseudoterminal");
    println!("with output on a pseudoterminal:");
    report(run_with_output(pty.slave, pty.master));
}

/// Run the script with its stdout on `write`, and read from `read`.
///
/// Returns what had arrived by the deadline, and what arrived in the end.
fn run_with_output(write: OwnedFd, read: OwnedFd) -> (String, String) {
    let program =
        CString::new(writer_path().as_os_str().as_encoded_bytes()).expect("no NUL in a path");
    let argv: Vec<*const libc::c_char> = vec![program.as_ptr(), std::ptr::null()];

    // SAFETY: the child calls only async-signal-safe functions — `dup2`,
    // `execv`, `_exit` — on pointers built before the fork.
    let child = match unsafe { fork() }.expect("fork failed") {
        ForkResult::Child => {
            let _ = dup2_stdout(&write);
            let _ = dup2_stderr(&write);

            // SAFETY: `argv` is NULL-terminated and alive in this address-space
            // copy. On success this does not return.
            unsafe { libc::execv(program.as_ptr(), argv.as_ptr()) };

            // SAFETY: `_exit` terminates without flushing inherited buffers.
            unsafe { libc::_exit(127) }
        }
        ForkResult::Parent { child } => child,
    };

    // The parent's copy of the write end has to go, or the read below never
    // reaches end-of-input.
    drop(write);

    let mut early = String::new();
    let mut everything = String::new();
    let deadline = Instant::now() + PEEK_AFTER;

    set_nonblocking(&read);
    let mut reader = std::fs::File::from(read);

    while Instant::now() < deadline {
        let mut chunk = [0_u8; 256];
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => early.push_str(&String::from_utf8_lossy(&chunk[..count])),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }

    everything.push_str(&early);
    let _ = waitpid(child, None);

    loop {
        let mut chunk = [0_u8; 256];
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => everything.push_str(&String::from_utf8_lossy(&chunk[..count])),
            Err(_) => break,
        }
    }

    (tidy(&early), tidy(&everything))
}

fn set_nonblocking(fd: &OwnedFd) {
    // SAFETY: `F_SETFL` takes an int; the descriptor is owned by the caller.
    unsafe {
        let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

/// Drop the carriage returns a terminal driver adds, so the two transcripts can
/// be compared as the same text.
///
/// The escaping is left to `{:?}` at the point of printing — doing it here as
/// well would produce doubled backslashes and quietly make the two runs look
/// different when they are not.
fn tidy(text: &str) -> String {
    text.replace('\r', "")
}

fn report((early, everything): (String, String)) {
    if early.is_empty() {
        println!("  after the pause: nothing yet");
    } else {
        println!("  after the pause: {early:?}");
    }
    println!("  in the end:      {everything:?}");
    println!();
}
