//! A shell running on the far side of a pseudoterminal.
//!
//! Job control and terminal handling cannot be observed without a terminal —
//! Ctrl-Z only exists because a terminal driver turns a keystroke into a signal
//! for the foreground process group, and terminal modes only matter because a
//! terminal has them. So the harness opens a pseudoterminal, runs `whelk` on the
//! far side of it, and types at it.

#![allow(dead_code)]

use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

use nix::pty::{forkpty, ForkptyResult};
use nix::sys::wait::waitpid;
use nix::unistd::Pid;

const WHELK: &str = env!("CARGO_BIN_EXE_whelk");

pub const CTRL_C: &str = "\x03";
pub const CTRL_Z: &str = "\x1a";

/// How long to wait for the shell to catch up after each keystroke.
const SETTLE: Duration = Duration::from_millis(500);

/// A shell running on the far side of a pseudoterminal.
pub struct Terminal {
    master: OwnedFd,
    child: Pid,
    seen: String,
}

impl Terminal {
    /// Start `whelk` with a pseudoterminal as its standard input and output.
    pub fn open() -> Self {
        Self::open_with_env(&[])
    }

    /// Start `whelk` with extra environment variables.
    ///
    /// Used to point `HOME` at a scratch directory, so tests of history and
    /// configuration cannot read or write the real one.
    pub fn open_with_env(vars: &[(&str, &str)]) -> Self {
        // Everything the child needs is built here, before the fork.
        //
        // Between `fork` and `exec` a child may call only async-signal-safe
        // functions, and the tests run on libtest's threads. `CString::new`
        // allocates; `set_var` takes the environment lock. Either one, forked
        // at the moment another thread held it, leaves the child waiting on a
        // lock whose owner does not exist in the child — the deadlock this
        // repository warns about in docs/process-model.md, and the reason the
        // suite hung on macOS: glibc reinitialises those locks across `fork`
        // and Apple's libc does not.
        //
        // So the environment is assembled as an `envp` for `execve` rather
        // than set with `set_var` in the child, and the child calls nothing
        // but `execve`. The shell itself follows this discipline; the harness
        // that tests it has to as well.
        let program = std::ffi::CString::new(WHELK).expect("no NUL in a path");

        let overridden = |name: &String| vars.iter().any(|(over, _)| name == *over);
        let environment: Vec<std::ffi::CString> = std::env::vars()
            .filter(|(name, _)| !overridden(name))
            .map(|(name, value)| format!("{name}={value}"))
            .chain(vars.iter().map(|(name, value)| format!("{name}={value}")))
            .map(|entry| std::ffi::CString::new(entry).expect("no NUL in an environment entry"))
            .collect();

        let argv = [program.as_ptr(), std::ptr::null()];
        let envp: Vec<_> = environment
            .iter()
            .map(|entry| entry.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();

        // SAFETY: `forkpty` forks. The child branch below does nothing but
        // `execve`, on arguments built above and inherited by the child.
        let result = unsafe { forkpty(None, None) }.expect("failed to open a pseudoterminal");

        match result {
            ForkptyResult::Child => {
                // SAFETY: `argv` and `envp` are NULL-terminated and were built
                // before the fork, so the child only reads memory it inherited
                // and allocates nothing. On success this does not return.
                unsafe { nix::libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr()) };

                // SAFETY: `_exit` terminates without flushing inherited buffers.
                unsafe { nix::libc::_exit(127) }
            }
            ForkptyResult::Parent { child, master } => {
                let mut terminal = Self {
                    master,
                    child,
                    seen: String::new(),
                };
                terminal.settle();
                terminal
            }
        }
    }

    /// Type something, then let the shell respond.
    pub fn type_in(&mut self, text: &str) -> &mut Self {
        let mut writer = std::fs::File::from(
            self.master
                .try_clone()
                .expect("failed to clone the pty master"),
        );
        writer
            .write_all(text.as_bytes())
            .expect("failed to write to the pty");
        drop(writer);
        self.settle();
        self
    }

    /// Read whatever the shell has produced, until it goes quiet.
    fn settle(&mut self) {
        set_nonblocking(&self.master);
        let deadline = Instant::now() + SETTLE;

        while Instant::now() < deadline {
            let mut chunk = [0_u8; 4096];
            let mut reader = std::fs::File::from(
                self.master
                    .try_clone()
                    .expect("failed to clone the pty master"),
            );

            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    // A terminal driver turns "\n" into "\r\n" on the way out.
                    // Dropping the carriage returns lets assertions read like
                    // ordinary text rather than encoding a detail of the device.
                    let text = String::from_utf8_lossy(&chunk[..count]);
                    self.seen.push_str(&text.replace('\r', ""));
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }

            std::mem::forget(reader);
        }
    }

    /// Everything the shell has printed so far.
    pub fn output(&self) -> &str {
        &self.seen
    }

    /// Resize the terminal, which sends the shell a `SIGWINCH`.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let size = nix::libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // SAFETY: `TIOCSWINSZ` reads a `winsize` through the pointer, and that
        // is what is passed. The kernel sends SIGWINCH to the foreground group
        // as a side effect, which is the point.
        unsafe {
            nix::libc::ioctl(
                self.master.as_raw_fd(),
                nix::libc::TIOCSWINSZ,
                &raw const size,
            );
        }

        self.settle();
    }

    /// Wait for text to appear, prompting the shell to look again if needed.
    ///
    /// Notifications about background jobs arrive at a *prompt*, so a test that
    /// typed once and asserted immediately would be racing the job. A blank
    /// line is a harmless way to ask for another prompt — it runs nothing and
    /// does not disturb `$?`.
    pub fn expect_eventually(&mut self, needle: &str) {
        for _ in 0..10 {
            if self.seen.contains(needle) {
                return;
            }
            self.type_in("\n");
        }

        panic!("expected {needle:?} in the session:\n{}", self.seen);
    }

    /// Assert that some text appeared.
    pub fn expect(&self, needle: &str) {
        assert!(
            self.seen.contains(needle),
            "expected {needle:?} in the session:\n{}",
            self.seen
        );
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Closing the master sends the shell an end-of-input, which is how a
        // terminal hanging up looks from the inside.
        let _ = nix::sys::signal::kill(self.child, nix::sys::signal::Signal::SIGKILL);
        let _ = waitpid(self.child, None);
    }
}

fn set_nonblocking(fd: &OwnedFd) {
    use nix::fcntl::{fcntl, FcntlArg, OFlag};
    let flags = fcntl(fd, FcntlArg::F_GETFL).unwrap_or(0);
    let flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
    let _ = fcntl(fd, FcntlArg::F_SETFL(flags));
}
