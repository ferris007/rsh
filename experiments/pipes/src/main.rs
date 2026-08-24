//! Experiment: what does a child inherit when the parent ignores a signal?
//!
//! See `README.md` in this directory for the question and the observation.
//!
//! The program execs the same tiny script twice — once leaving the inherited
//! disposition alone, once resetting SIGPIPE to its default — and reports how
//! each child died. The script asks to be killed by SIGPIPE, so the answer is
//! visible in the exit status without needing a pipe at all.

use std::ffi::CString;

use nix::libc;
use nix::sys::signal::Signal;
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{fork, ForkResult};

/// A script that asks the kernel to deliver SIGPIPE to the child itself.
///
/// Using `kill` rather than a real broken pipe keeps the result deterministic:
/// no timing, no buffer sizes, just the disposition.
const SCRIPT: &str = "kill -PIPE $$";

fn main() {
    println!("this process ignores SIGPIPE: {}", ignoring_sigpipe());
    println!("(Rust's runtime sets that at startup, before main)");
    println!();

    println!("child with the inherited disposition:");
    report(run_child(false));

    println!("child after resetting SIGPIPE to SIG_DFL:");
    report(run_child(true));
}

/// Whether this process is currently ignoring SIGPIPE.
fn ignoring_sigpipe() -> bool {
    // SAFETY: installing SIG_IGN and immediately putting back whatever was
    // there. `signal` returns the previous disposition, which is the only thing
    // being asked for.
    let previous = unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    // SAFETY: restoring the disposition read above.
    unsafe { libc::signal(libc::SIGPIPE, previous) };
    previous == libc::SIG_IGN
}

/// Fork, optionally reset SIGPIPE, and exec a script that signals itself.
fn run_child(reset: bool) -> WaitStatus {
    let sh = CString::new("/bin/sh").expect("no NUL in a literal");
    let dash_c = CString::new("-c").expect("no NUL in a literal");
    let script = CString::new(SCRIPT).expect("no NUL in a literal");
    let argv: Vec<*const libc::c_char> = vec![
        sh.as_ptr(),
        dash_c.as_ptr(),
        script.as_ptr(),
        std::ptr::null(),
    ];

    // SAFETY: the child calls only async-signal-safe functions — `signal`,
    // `execv`, `_exit` — on pointers built before the fork.
    match unsafe { fork() }.expect("fork failed") {
        ForkResult::Child => {
            if reset {
                // SAFETY: `signal` is async-signal-safe, and setting a
                // disposition rather than a handler means no Rust code can run
                // in a signal context.
                unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
            }

            // SAFETY: `argv` is NULL-terminated and alive in this address-space
            // copy. On success this does not return.
            unsafe { libc::execv(sh.as_ptr(), argv.as_ptr()) };

            // SAFETY: `_exit` terminates without flushing inherited buffers.
            unsafe { libc::_exit(127) }
        }
        ForkResult::Parent { child } => waitpid(child, None).expect("waitpid failed"),
    }
}

fn report(status: WaitStatus) {
    match status {
        WaitStatus::Signaled(_, Signal::SIGPIPE, _) => {
            println!("  killed by SIGPIPE — the default action reached it");
        }
        WaitStatus::Exited(_, code) => {
            println!("  exited normally with {code} — the signal was ignored");
        }
        other => println!("  unexpected: {other:?}"),
    }
}
