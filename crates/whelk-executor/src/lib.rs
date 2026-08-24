//! Running what the parser produced.
//!
//! The executor owns the shell's mutable state — the last exit status, the
//! current directory, the environment — and decides, for each command, whether
//! it is something the shell must do itself or something a child process does.
//!
//! It receives an AST and reads the source line only to underline errors — the
//! spans in the tree say which characters to point at, never what they mean.
//!
//! Expansion lives here rather than in the parser, because resolving `$HOME`
//! means reading the environment. See [`expand`] for the argument.

mod builtin;
mod expand;
mod pipeline;
mod redirect;
mod shell;

pub use expand::{expand_all, expand_one, AmbiguousRedirect, Environment, MapEnv, ProcessEnv};
pub use pipeline::PipelineError;
pub use redirect::RedirectError;
pub use shell::{Outcome, Shell};
