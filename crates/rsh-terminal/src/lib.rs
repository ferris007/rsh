//! The terminal.
//!
//! A terminal is not a file, however much the descriptor pretends otherwise. It
//! is a device with modes, a size, an owner, and — crucially — **state that
//! outlives the process that changed it**. A program that puts the terminal in
//! raw mode and dies leaves it in raw mode, and the shell that started it is
//! the only thing left able to put it back.
//!
//! That is what this crate is for. Three separate concerns share the word
//! "terminal", and keeping them apart is most of the work:
//!
//! * **Ownership** — which process group may read from it, and receives its
//!   signals. Set with `tcsetpgrp`; the foundation of job control.
//! * **Modes** — whether the driver echoes, buffers into lines, and turns
//!   Ctrl-C into a signal. Set with `tcsetattr`; what raw mode changes.
//! * **Size** — rows and columns, which change under the program's feet and
//!   announce themselves with `SIGWINCH`.
//!
//! # The invariant
//!
//! Whatever a job does to the terminal, the shell puts it back before printing
//! its next prompt. Not because jobs are untrustworthy, but because a job that
//! is suspended mid-`vim` has legitimately left the terminal in a state the
//! shell cannot use — and a job killed mid-`vim` has left it that way with
//! nobody to notice.

mod modes;
mod ownership;
mod size;

pub use modes::{raw, restore, snapshot, Modes, RawGuard};
pub use ownership::{foreground_group, give_to, is_interactive};
pub use size::{size, Size};
