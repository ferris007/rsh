//! Anonymous pipes.
//!
//! A pipe is a kernel buffer with two descriptors attached: everything written
//! to one end can be read from the other. It has no name, no place in the
//! filesystem, and no existence beyond the processes holding those
//! descriptors — which is exactly why it is the right connector for a shell
//! pipeline and the wrong one for anything that has to outlive it.
//!
//! Two properties do all the work in a pipeline:
//!
//! * **The reader sees end-of-input only when every write end is closed.** Not
//!   most of them — all. A single forgotten copy in the shell keeps `grep`
//!   waiting forever, which is the classic way to hang a pipeline.
//! * **Writes block when the buffer is full.** That is backpressure, and it is
//!   why `find / | head -1` does not have to buffer the filesystem.

use std::os::fd::OwnedFd;

use nix::fcntl::{fcntl, FcntlArg, FdFlag};
use nix::unistd::pipe as sys_pipe;

use crate::command::SpawnError;

/// The two ends of an anonymous pipe.
#[derive(Debug)]
pub struct Pipe {
    /// The end a process reads from.
    pub read: OwnedFd,
    /// The end a process writes to.
    pub write: OwnedFd,
}

impl Pipe {
    /// Create a pipe whose ends are closed automatically by `exec`.
    ///
    /// Close-on-exec is what makes a pipeline's bookkeeping tractable. Every
    /// child inherits a copy of *every* pipe in the pipeline, and each needs to
    /// keep only the one or two it was given. Rather than closing the rest by
    /// hand — in the child, where a mistake cannot be reported — the flag lets
    /// `exec` do it: `dup2` clears the flag on the descriptors the child is
    /// meant to keep, and everything still carrying it disappears.
    ///
    /// The flag is set after the fact rather than by `pipe2`, which does not
    /// exist on macOS. The gap between creating the descriptors and marking
    /// them would matter in a program that forks from another thread; `whelk` is
    /// single-threaded, and the alternative is a platform-specific fast path
    /// for a syscall that is not on the hot side of anything.
    pub fn new() -> Result<Self, SpawnError> {
        let (read, write) = sys_pipe().map_err(SpawnError::Pipe)?;

        for end in [&read, &write] {
            fcntl(end, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(SpawnError::Pipe)?;
        }

        Ok(Self { read, write })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;

    #[test]
    fn what_goes_in_one_end_comes_out_the_other() {
        let pipe = Pipe::new().expect("failed to create a pipe");
        let mut writer = std::fs::File::from(pipe.write);
        let mut reader = std::fs::File::from(pipe.read);

        writer.write_all(b"hello").expect("failed to write");
        drop(writer);

        let mut received = String::new();
        reader
            .read_to_string(&mut received)
            .expect("failed to read");
        assert_eq!(received, "hello");
    }

    #[test]
    fn the_reader_sees_end_of_input_when_the_write_end_closes() {
        // The whole reason a pipeline has to be careful about stray copies of
        // the write end: this read would block forever if one were still open.
        let pipe = Pipe::new().expect("failed to create a pipe");
        drop(pipe.write);

        let mut reader = std::fs::File::from(pipe.read);
        let mut received = Vec::new();
        assert_eq!(
            reader.read_to_end(&mut received).expect("failed to read"),
            0
        );
    }

    #[test]
    fn both_ends_are_close_on_exec() {
        let pipe = Pipe::new().expect("failed to create a pipe");
        for end in [&pipe.read, &pipe.write] {
            let flags = fcntl(end, FcntlArg::F_GETFD).expect("failed to read descriptor flags");
            assert!(FdFlag::from_bits_truncate(flags).contains(FdFlag::FD_CLOEXEC));
        }
    }

    #[test]
    fn the_ends_are_distinct_descriptors() {
        let pipe = Pipe::new().expect("failed to create a pipe");
        assert_ne!(pipe.read.as_raw_fd(), pipe.write.as_raw_fd());
    }
}
