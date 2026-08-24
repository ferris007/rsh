//! Experiment: what does `dup2(fd, fd)` do?
//!
//! See `README.md` in this directory for the question and the observation.
//!
//! The program opens a file, inspects its close-on-exec flag, applies `dup2`
//! both ways, and then actually `exec`s a program that writes to the
//! descriptor — because the flag is only interesting for what it does at
//! `exec` time.

use std::ffi::CString;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, RawFd};

use nix::libc;
use nix::sys::wait::waitpid;
use nix::unistd::{fork, ForkResult};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: xp-file-descriptors <file>");
        eprintln!();
        eprintln!("Opens <file>, reports its close-on-exec flag through two");
        eprintln!("forms of dup2, then execs a program that writes to it.");
        std::process::exit(2);
    };

    // `std::fs` sets close-on-exec on everything it opens, which is what a
    // shell wants: the descriptor it opened for a redirection should not also
    // arrive in the child under its original number.
    let file = open(&path);
    let fd = file.as_raw_fd();
    println!("opened as fd {fd}, FD_CLOEXEC: {}", cloexec(fd));

    // The surprise. POSIX says `dup2(fd, fd)` returns fd and does nothing
    // else — which includes not clearing FD_CLOEXEC.
    let same = dup2(fd, fd);
    println!(
        "after dup2({fd}, {fd}) -> {same}, FD_CLOEXEC: {}",
        cloexec(fd)
    );

    // Any *other* target gets a fresh copy, and a copy never inherits the
    // flag: FD_CLOEXEC belongs to the descriptor, not to the open file.
    let copy = dup2(fd, fd + 1);
    println!(
        "after dup2({fd}, {}) -> {copy}, FD_CLOEXEC: {}",
        fd + 1,
        cloexec(fd + 1)
    );

    // And the consequence, which is the part that matters. A child that execs
    // only keeps descriptors whose flag is clear.
    let script = format!("echo written-to-{fd} >&{fd}");
    println!("exec with the flag left as-is:");
    report(run_child(&script, fd, false));

    println!("exec with the flag cleared first:");
    report(run_child(&script, fd, true));

    println!(
        "file now contains: {:?}",
        std::fs::read_to_string(&path).unwrap_or_default()
    );
}

fn open(path: &str) -> File {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .expect("failed to open the experiment file")
}

/// Whether close-on-exec is set on a descriptor.
fn cloexec(fd: RawFd) -> &'static str {
    // SAFETY: `F_GETFD` only reads flags. An invalid descriptor returns -1
    // rather than doing anything undefined.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    match flags {
        f if f < 0 => "error",
        f if f & libc::FD_CLOEXEC != 0 => "set",
        _ => "clear",
    }
}

fn dup2(source: RawFd, target: RawFd) -> RawFd {
    // SAFETY: both arguments are descriptors owned by this process; a stale one
    // would fail with EBADF rather than corrupt anything.
    unsafe { libc::dup2(source, target) }
}

/// Fork, optionally clear the flag, and exec a shell that writes to `fd`.
///
/// Returns the child's exit status. Everything that allocates — building the
/// C strings — happens before the fork, as it must.
fn run_child(script: &str, fd: RawFd, clear_flag: bool) -> i32 {
    let sh = CString::new("/bin/sh").expect("no NUL in a literal");
    let dash_c = CString::new("-c").expect("no NUL in a literal");
    let script = CString::new(script).expect("script contains a NUL");
    let argv: Vec<*const libc::c_char> = vec![
        sh.as_ptr(),
        dash_c.as_ptr(),
        script.as_ptr(),
        std::ptr::null(),
    ];

    // SAFETY: the child below calls only async-signal-safe functions —
    // `fcntl`, `execv`, `_exit` — on pointers built above, before the fork.
    match unsafe { fork() }.expect("fork failed") {
        ForkResult::Child => {
            if clear_flag {
                // SAFETY: `fcntl` is async-signal-safe. Zeroing the descriptor
                // flags clears FD_CLOEXEC, the only flag defined for them.
                unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
            }

            // SAFETY: `argv` is NULL-terminated and its pointers are alive in
            // this address-space copy. On success this does not return.
            unsafe { libc::execv(sh.as_ptr(), argv.as_ptr()) };

            // SAFETY: `_exit` terminates without flushing inherited buffers.
            unsafe { libc::_exit(127) }
        }
        ForkResult::Parent { child } => match waitpid(child, None).expect("waitpid failed") {
            nix::sys::wait::WaitStatus::Exited(_, code) => code,
            other => panic!("unexpected child status: {other:?}"),
        },
    }
}

fn report(status: i32) {
    if status == 0 {
        println!("  child succeeded: the descriptor survived exec");
    } else {
        println!("  child failed with {status}: the descriptor was closed by exec");
    }
}
