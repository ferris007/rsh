//! Experiment: why the fork/exec child must call `_exit`, not `exit`.
//!
//! See `README.md` in this directory for the question and the observation.
//!
//! The program writes a marker to stdout *without* a trailing newline, so it
//! stays in the buffer, then forks. The child exits — one way or the other —
//! and the parent then flushes. Counting how many markers reach stdout answers
//! the question.

use std::io::Write;

use nix::libc;
use nix::sys::wait::waitpid;
use nix::unistd::{fork, ForkResult};

/// Written to stdout without a newline, so it stays in the buffer across the
/// fork. Rust's stdout is line-buffered even when it is a pipe, which is what
/// makes this reproducible under a test harness.
const MARKER: &str = "[buffered]";

/// How the child should terminate.
#[derive(Debug, Clone, Copy)]
enum Mode {
    /// `exit(3)`: runs `atexit` handlers, which flush stdio.
    Exit,
    /// `_exit(2)`: returns to the kernel immediately.
    UnderscoreExit,
}

fn main() {
    let mode = match std::env::args().nth(1).as_deref() {
        Some("exit") => Mode::Exit,
        Some("_exit") => Mode::UnderscoreExit,
        _ => {
            eprintln!("usage: xp-fork-exec <exit|_exit>");
            eprintln!();
            eprintln!("Writes an unterminated line to stdout, forks, and has the");
            eprintln!("child terminate the chosen way. Count the markers on stdout.");
            std::process::exit(2);
        }
    };

    // No newline: this sits in the process's stdout buffer, unwritten.
    print!("{MARKER}");

    // SAFETY: `fork` is unsafe because of what the child may do afterwards.
    // This child does one thing — terminate — and the whole point of the
    // experiment is to observe what that one thing drags along with it.
    match unsafe { fork() }.expect("fork failed") {
        ForkResult::Child => match mode {
            // Flushes the *copy* of the parent's buffer that the child
            // inherited. Nothing here wrote that text; the child merely
            // inherited the obligation to flush it.
            Mode::Exit => std::process::exit(0),

            // SAFETY: `_exit` is async-signal-safe and terminates immediately,
            // without running `atexit` handlers or touching stdio.
            Mode::UnderscoreExit => unsafe { libc::_exit(0) },
        },

        ForkResult::Parent { child } => {
            // Wait first, so the child's output (if any) lands before ours and
            // the result is deterministic rather than a race.
            waitpid(child, None).expect("waitpid failed");

            // Terminate the line, which flushes the parent's own buffer.
            println!();
            let _ = std::io::stdout().flush();

            eprintln!("child terminated with {mode:?}; count the markers on stdout");
        }
    }
}
