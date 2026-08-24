//! The program under observation.
//!
//! It writes a line, pauses, writes another, and exits — using C's `printf`
//! rather than Rust's, because the buffering rule being demonstrated belongs to
//! C's standard library. `libc` decides at first use whether this program's
//! stdout is line-buffered or fully buffered, and it decides by asking whether
//! the descriptor is a terminal.
//!
//! Rust's own `println!` would show nothing: `std::io::Stdout` is a
//! `LineWriter` whatever it is attached to, which is a deliberate departure
//! from C and one of the few places Rust quietly fixes a decades-old footgun.

use std::ffi::CString;

use nix::libc;

fn main() {
    let first = CString::new("first\n").expect("no NUL in a literal");
    let second = CString::new("second\n").expect("no NUL in a literal");

    // SAFETY: `printf` with a NUL-terminated format string and no arguments.
    unsafe { libc::printf(first.as_ptr()) };

    // Long enough that an observer can see whether the line above has arrived.
    // SAFETY: `sleep` takes a count of seconds.
    unsafe { libc::sleep(1) };

    // SAFETY: as above.
    unsafe { libc::printf(second.as_ptr()) };

    // Returning from `main` runs the C library's exit handlers, which flush.
    // That flush is what makes the fully-buffered case arrive all at once.
}
