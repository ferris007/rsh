//! Creating processes and finding out how they ended.
//!
//! This crate is the boundary between `whelk` and the process table. It holds
//! every `unsafe` block in the shell that touches `fork`, and it is small on
//! purpose: the correctness argument for the fork/exec window has to fit in
//! one reading. See `docs/process-model.md`.

mod command;
mod path;
mod pipe;
mod redirect;
mod signal;
mod status;

pub use command::{Child, Command, SpawnError, Waited};
pub use path::{resolve, suggest, ResolveError};
pub use pipe::Pipe;
pub use redirect::{Redirections, Restore};
pub use signal::{
    drain_notifications, install as install_signal_handlers, notify_fd, shutdown_requested,
    take_child_event, take_interrupt, take_resize,
};
pub use status::{
    collect_child_events, collect_child_events_blocking, ChildEvent, ExitStatus,
    EXIT_NOT_EXECUTABLE, EXIT_NOT_FOUND, EXIT_SIGNAL_BASE,
};
