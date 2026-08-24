//! Job control, driven through a pseudoterminal.
//!
//! These tests need a terminal, and not as a convenience: job control *is*
//! terminal ownership. Ctrl-Z only exists because a terminal driver turns a
//! keystroke into a signal for the foreground process group, and `fg` only
//! means anything because there is a foreground slot to hand over. A shell
//! reading from a pipe has none of that and correctly refuses to pretend.
//!
//! So the harness opens a pseudoterminal, runs `rsh` on the far side of it, and
//! types at it — which is the only way to observe the behaviour this phase
//! adds.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use nix::pty::{forkpty, ForkptyResult};
use nix::sys::wait::waitpid;
use nix::unistd::Pid;

const RSH: &str = env!("CARGO_BIN_EXE_rsh");

const CTRL_C: &str = "\x03";
const CTRL_Z: &str = "\x1a";

/// How long to wait for the shell to catch up after each keystroke.
const SETTLE: Duration = Duration::from_millis(500);

/// A shell running on the far side of a pseudoterminal.
struct Terminal {
    master: OwnedFd,
    child: Pid,
    seen: String,
}

impl Terminal {
    /// Start `rsh` with a pseudoterminal as its standard input and output.
    fn open() -> Self {
        // SAFETY: `forkpty` forks. The child branch below does nothing but
        // `execv` on a path built before the call, which is the same discipline
        // the shell itself follows.
        let result = unsafe { forkpty(None, None) }.expect("failed to open a pseudoterminal");

        match result {
            ForkptyResult::Child => {
                let program = std::ffi::CString::new(RSH).expect("no NUL in a path");
                let argv = [program.as_ptr(), std::ptr::null()];

                // SAFETY: `argv` is NULL-terminated and alive here. On success
                // this does not return.
                unsafe { nix::libc::execv(program.as_ptr(), argv.as_ptr()) };

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
    fn type_in(&mut self, text: &str) -> &mut Self {
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
                    self.seen
                        .push_str(&String::from_utf8_lossy(&chunk[..count]));
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
    fn output(&self) -> &str {
        &self.seen
    }

    /// Assert that some text appeared.
    fn expect(&self, needle: &str) {
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

// ---- background jobs -------------------------------------------------------

#[test]
fn a_background_job_is_announced_and_listed() {
    let mut session = Terminal::open();
    session.type_in("sleep 30 &\n");
    session.expect("[1]");

    session.type_in("jobs\n");
    session.expect("Running");
    session.expect("sleep 30");
}

#[test]
fn a_finished_background_job_is_reported_at_the_next_prompt() {
    // Not the instant it happens: a notification arriving mid-keystroke would
    // write over whatever the user was typing.
    let mut session = Terminal::open();
    session.type_in("sleep 0.2 &\n");
    session.type_in("\n");
    session.expect("Done");
}

// ---- suspend and resume ----------------------------------------------------

#[test]
fn ctrl_z_suspends_a_foreground_command() {
    let mut session = Terminal::open();
    session.type_in("sleep 30\n");
    session.type_in(CTRL_Z);
    session.expect("Stopped");

    session.type_in("jobs\n");
    session.expect("Stopped");
}

#[test]
fn bg_resumes_a_stopped_job_without_the_terminal() {
    let mut session = Terminal::open();
    session.type_in("sleep 30\n");
    session.type_in(CTRL_Z);
    session.expect("Stopped");

    session.type_in("bg\n");
    session.type_in("jobs\n");
    session.expect("Running");
}

#[test]
fn fg_brings_a_stopped_job_back_and_waits_for_it() {
    let mut session = Terminal::open();

    // Long enough that it is still there when Ctrl-Z arrives. A short sleep
    // would finish first and the test would suspend an empty prompt instead.
    session.type_in("sleep 30\n");
    session.type_in(CTRL_Z);
    session.expect("Stopped");

    // `fg` echoes the command it is resuming, then blocks — which is the half
    // that distinguishes it from `bg`.
    session.type_in("fg\n");
    session.expect("sleep 30");

    // The shell is waiting, so Ctrl-C has to go to the job to get it back.
    session.type_in(CTRL_C);
    session.type_in("echo back\n");
    session.expect("back");

    // Having been waited for, the job is gone rather than left in the table.
    session.type_in("jobs\n");
    session.type_in("echo listed\n");
    session.expect("listed");
}

// ---- process groups --------------------------------------------------------

#[test]
fn ctrl_c_kills_the_foreground_job_and_leaves_the_shell() {
    let mut session = Terminal::open();
    session.type_in("sleep 30\n");
    session.type_in(CTRL_C);

    // The shell is still there to answer.
    session.type_in("echo status=$?\n");
    session.expect("status=130");
}

#[test]
fn ctrl_c_does_not_reach_a_background_job() {
    // The whole point of giving each job its own process group. The signal goes
    // to the foreground group, and a background job is not in it.
    let mut session = Terminal::open();
    session.type_in("sleep 30 &\n");
    session.type_in("sleep 30\n");
    session.type_in(CTRL_C);

    session.type_in("jobs\n");
    session.expect("Running");
}

// ---- leaving ---------------------------------------------------------------

#[test]
fn exiting_with_a_stopped_job_warns_first() {
    // A stopped job would be left suspended with nothing able to resume it.
    // The shell states the consequence; the decision stays with the user.
    let mut session = Terminal::open();
    session.type_in("sleep 30\n");
    session.type_in(CTRL_Z);
    session.expect("Stopped");

    session.type_in("exit\n");
    session.expect("there are stopped jobs");

    // Still running, having declined to leave.
    session.type_in("echo still here\n");
    session.expect("still here");
}

#[test]
fn a_running_background_job_does_not_block_exit() {
    // It carries on perfectly well without a shell; only a *stopped* job is a
    // process leaked in a state the user cannot see.
    let mut session = Terminal::open();
    session.type_in("sleep 30 &\n");
    session.type_in("exit\n");
    assert!(
        !session.output().contains("stopped jobs"),
        "a running job should not block exit:\n{}",
        session.output()
    );
}
