//! The job table.
//!
//! A **job** is one pipeline: the processes it started, the process group they
//! share, and what the shell last knew about their state. The group is the
//! important part — it is what lets the shell signal, suspend, or resume an
//! entire pipeline as one thing without knowing how many stages it had.
//!
//! ```text
//!   [1]+  Running    sleep 30 | cat        pgid 4242
//!   [2]-  Stopped    vim notes.txt         pgid 4251
//! ```
//!
//! # Why this is its own crate
//!
//! The job table is not a step in the path from text to process. It is
//! long-lived state that outlives the command that created it, and both the
//! executor and the REPL consult it. Putting it beside them rather than inside
//! either one keeps it from being reached for accidentally — and keeps it
//! extractable, since "manage a group of child processes" is a problem far more
//! programs have than have a shell to solve it with.

mod job;
mod spec;
mod table;

pub use job::{Job, JobId, JobState};
pub use spec::JobSpec;
pub use table::JobTable;
