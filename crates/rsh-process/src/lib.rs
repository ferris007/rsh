//! Creating processes and finding out how they ended.
//!
//! This crate is the boundary between `rsh` and the process table. It holds
//! every `unsafe` block in the shell that touches `fork`, and it is small on
//! purpose: the correctness argument for the fork/exec window has to fit in
//! one reading. See `docs/process-model.md`.

mod command;
mod path;
mod redirect;
mod status;

pub use command::{Child, Command, SpawnError};
pub use path::{resolve, ResolveError};
pub use redirect::{Redirections, Restore};
pub use status::{ExitStatus, EXIT_NOT_EXECUTABLE, EXIT_NOT_FOUND, EXIT_SIGNAL_BASE};
