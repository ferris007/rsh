//! Line editing.
//!
//! This is the layer the roadmap says to keep away from the process core, and
//! the separation is load-bearing: nothing here spawns, signals, or waits for
//! anything. It turns keystrokes into a line of text.
//!
//! # Why it is testable without a terminal
//!
//! The editor performs no I/O. It is fed [`Key`] values and returns an
//! [`Action`], and rendering is a pure function from its state to a string of
//! bytes. The caller does the reading and writing.
//!
//! That is not a stylistic preference. A line editor is mostly edge cases —
//! word deletion at the start of a line, history navigation with a half-typed
//! line held back, completion of a word that is not the last one — and every
//! one of them is a two-line test when the editor is a state machine, and a
//! pseudoterminal session when it is not.

mod editor;
mod history;
mod keys;
mod render;

pub use editor::{Action, Completer, Completion, Editor, NoCompletion};
pub use history::History;
pub use keys::{decode, Decoded, Key};
pub use render::{line as render_line, suggestions as render_suggestions, CLEAR_SCREEN};
