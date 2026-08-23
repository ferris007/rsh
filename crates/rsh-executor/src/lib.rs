//! Running what the parser produced.
//!
//! The executor owns the shell's mutable state — the last exit status, the
//! current directory, the environment — and decides, for each command, whether
//! it is something the shell must do itself or something a child process does.
//!
//! It receives an AST and never sees the source text. If it ever needs to, the
//! AST is missing something.

mod builtin;
mod shell;

pub use shell::{Outcome, Shell};
