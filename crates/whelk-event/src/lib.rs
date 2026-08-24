//! Waiting for several things at once.
//!
//! Until now the shell has waited for exactly one thing at a time: `read` on
//! standard input, or `waitpid` on a child. That works because it always knew
//! which one it was waiting for — and it is why a background job finishing
//! goes unnoticed until the next prompt, and why a window resize is not seen
//! until the user presses Enter.
//!
//! An event loop inverts that. The shell says "wake me when *any* of these has
//! something to say" and is told which.
//!
//! ```text
//!         ┌──────────────┐
//!         │    Poller    │   epoll on Linux, kqueue on BSD and macOS
//!         └──────┬───────┘
//!      ┌─────────┼──────────┐
//!      ▼         ▼          ▼
//!    stdin    signals    (later: children, timers)
//! ```
//!
//! # What this is, in one sentence
//!
//! This crate is a very small [`mio`], which is the layer Tokio is built on.
//! Writing it is the point: an async runtime is a scheduler on top of exactly
//! this, and it is much easier to reason about the scheduler once the thing
//! underneath it is not mysterious.
//!
//! # Signals are not descriptors
//!
//! Neither `epoll` nor `kqueue` can wait for a signal in a portable way, and a
//! flag set by a handler is not enough on its own — the signal can arrive
//! between the check and the wait, leaving the loop blocked with the flag
//! already set and nothing left to wake it.
//!
//! The fix is the **self-pipe trick**: the handler writes one byte to a pipe,
//! and the pipe is an ordinary descriptor the poller can watch. A byte written
//! before the wait begins is still there when it does. See
//! `experiments/epoll`, which demonstrates the race the pipe closes.
//!
//! [`mio`]: https://docs.rs/mio

mod poll;
mod token;

pub use poll::{Event, Poller};

/// The error a wait returns when a signal arrived during it.
///
/// Re-exported so a caller can recognise it without depending on `nix` for one
/// constant — and it is the one error every event loop has to handle.
pub const INTERRUPTED: nix::errno::Errno = nix::errno::Errno::EINTR;
pub use token::Token;
