//! Benchmarks for `rsh`.
//!
//! A package of its own so that `criterion` — a large dependency tree next to a
//! workspace whose only other one wraps libc — stays out of the shell's own
//! graph entirely. Nothing here is compiled when the shell is.
//!
//! Run with `cargo bench`. See `docs/performance.md` for what the numbers mean
//! and which of them are worth watching.
