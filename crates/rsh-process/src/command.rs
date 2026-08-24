//! `fork` + `exec`, and the window between them.
//!
//! Read `docs/process-model.md` before changing anything here. The short
//! version: after `fork` returns `0`, the child holds a copy of the parent's
//! address space but only one thread. Any lock another thread held at that
//! instant — including the allocator's — is now held by a thread that does not
//! exist, and will never be released. POSIX therefore allows only
//! async-signal-safe calls between `fork` and `exec`.
//!
//! So the child here does three things and nothing else: `execv`, a `write` of
//! a constant byte string, and `_exit`. All three are async-signal-safe. Every
//! allocation, every fallible conversion, and every lookup happens in
//! [`Command::new`], before the fork.

use std::ffi::{CString, NulError};
use std::fmt;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use nix::errno::Errno;
use nix::libc;
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{fork, ForkResult, Pid};

use crate::redirect::Redirections;
use crate::status::{ExitStatus, EXIT_NOT_EXECUTABLE};

/// Reported by a child that could not rearrange its descriptors.
///
/// Distinct from an exec failure: the program was never reached. Most
/// redirection problems — a missing file, a permission error — are caught in
/// the parent, so this covers only what cannot be seen until the child runs.
const EXIT_REDIRECT_FAILED: i32 = 1;

/// Why a process could not be started, or its result collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    /// A path or argument contained an interior NUL byte.
    ///
    /// C strings cannot represent one, so this can never reach `exec`. Rust
    /// strings can hold it, which is why the conversion is fallible and why it
    /// is done in the parent where it can still be reported.
    InteriorNul { what: String },
    /// `fork` failed — typically `EAGAIN` from a process or memory limit.
    Fork(Errno),
    /// `waitpid` failed.
    Wait(Errno),
    /// A pipe could not be created — typically the descriptor limit.
    Pipe(Errno),
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InteriorNul { what } => write!(f, "{what} contains a NUL byte"),
            Self::Fork(e) => write!(f, "fork failed: {e}"),
            Self::Wait(e) => write!(f, "wait failed: {e}"),
            Self::Pipe(e) => write!(f, "cannot create pipe: {e}"),
        }
    }
}

impl std::error::Error for SpawnError {}

/// A program, prepared for execution.
///
/// Everything `exec` needs is built here, in the parent: the path as a C
/// string, the arguments as C strings, and the NULL-terminated pointer array
/// that `execv` actually reads. Constructing the pointer array in advance is
/// the point of this type — `nix`'s safe `execv` wrapper builds it from a
/// slice at call time, which allocates, and allocating in the child is the one
/// thing the fork window forbids.
#[derive(Debug)]
pub struct Command {
    /// The resolved path to execute.
    path: CString,
    /// Owns the bytes the pointers in `argv_ptrs` point at.
    ///
    /// Never mutated after construction. The pointers remain valid if this
    /// struct moves — a `CString`'s bytes live on the heap, so moving the
    /// `Vec` moves only its header — but they would dangle if an element were
    /// replaced or the vector reallocated. There is no method that does either.
    _argv: Vec<CString>,
    /// `argv` as `exec` wants it: pointers, NULL-terminated.
    argv_ptrs: Vec<*const libc::c_char>,
    /// Descriptor changes to make in the child, in order.
    redirections: Redirections,
    /// The process group to join, if the shell is doing job control.
    ///
    /// `Some(None)` means "lead a new group"; `Some(Some(pgid))` means "join
    /// this one". `None` means leave the group alone, which is what a shell
    /// without job control does.
    process_group: Option<Option<Pid>>,
}

impl Command {
    /// Prepare a resolved program path and its argument vector.
    ///
    /// `argv` must include the program name as its first element: that is what
    /// `argv[0]` means to the program being run, and several programs
    /// (`busybox`, `vi` vs `vim`) change behaviour based on it.
    pub fn new<I, S>(path: &Path, argv: I) -> Result<Self, SpawnError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let path_c = CString::new(path.as_os_str().as_bytes()).map_err(|_: NulError| {
            SpawnError::InteriorNul {
                what: "program path".to_owned(),
            }
        })?;

        let argv: Vec<CString> = argv
            .into_iter()
            .map(|arg| {
                CString::new(arg.as_ref()).map_err(|_: NulError| SpawnError::InteriorNul {
                    what: format!("argument `{}`", arg.as_ref().escape_debug()),
                })
            })
            .collect::<Result<_, _>>()?;

        let mut argv_ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());

        Ok(Self {
            path: path_c,
            _argv: argv,
            argv_ptrs,
            redirections: Redirections::new(),
            process_group: None,
        })
    }

    /// Put the child in a process group.
    ///
    /// `None` makes it the leader of a new group whose id is its own pid;
    /// `Some(pgid)` joins an existing one. Every stage of a pipeline joins the
    /// first stage's group, which is what makes one Ctrl-C reach all of them.
    ///
    /// The call is made in *both* parent and child, deliberately. Either may be
    /// scheduled first after the fork, and the group has to exist before
    /// anything can signal it or hand it the terminal — so both do it and one
    /// of the two calls is always redundant. Which one is not knowable in
    /// advance, so neither can be dropped.
    #[must_use]
    pub fn process_group(mut self, pgid: Option<Pid>) -> Self {
        self.process_group = Some(pgid);
        self
    }

    /// Attach descriptor changes to apply in the child.
    ///
    /// They are applied after the fork and before the exec — the only moment
    /// when a process's descriptors can be rearranged without disturbing the
    /// shell's own.
    #[must_use]
    pub fn redirections(mut self, redirections: Redirections) -> Self {
        self.redirections = redirections;
        self
    }

    /// Fork, and execute the program in the child.
    ///
    /// Returns in the parent with a handle to the new process. The child never
    /// returns from this function: it either becomes the target program or
    /// exits.
    pub fn spawn(&self) -> Result<Child, SpawnError> {
        // SAFETY: `fork` is unsafe because of what the child may do afterwards,
        // not because of the call itself. The child branch below performs only
        // async-signal-safe operations (`execv`, `write`, `_exit`) on data that
        // was fully prepared before the fork, so it cannot deadlock on a lock
        // held by a thread that did not survive into the child.
        match unsafe { fork() } {
            Ok(ForkResult::Parent { child }) => Ok(Child { pid: child }),

            Ok(ForkResult::Child) => {
                // ---- async-signal-safe territory begins here ----

                // Join the process group before anything else, so that a
                // signal aimed at the job cannot arrive while this process is
                // still in the shell's group.
                //
                // SAFETY: `setpgid` is async-signal-safe. A failure here is not
                // worth aborting for — the process runs, it is simply in the
                // wrong group — and there is no way to report it from here.
                if let Some(pgid) = self.process_group {
                    let target = pgid.map_or(0, Pid::as_raw);
                    // SAFETY: `setpgid` is async-signal-safe. A failure is not
                    // worth aborting for — the process runs, it is simply in
                    // the wrong group — and there is no way to report it here.
                    unsafe { libc::setpgid(0, target) };
                }

                // Put the job-control signals back to their defaults.
                //
                // The shell ignores SIGTSTP, SIGTTIN, and SIGTTOU so that it
                // cannot suspend itself while moving the terminal around. But
                // SIG_IGN is inherited across exec, so without this every child
                // would be immune to Ctrl-Z — the shell's self-protection would
                // silently become a property of every program it runs.
                //
                // This is the same trap SIGPIPE sets below, and the reason the
                // shell uses handlers for SIGINT and SIGQUIT: exec resets those
                // for free.
                for signal in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
                    // SAFETY: `signal` is async-signal-safe, and SIG_DFL
                    // installs no code that could run in a signal context.
                    unsafe { libc::signal(signal, libc::SIG_DFL) };
                }

                // Descriptors first: the program must start with the ones the
                // user asked for. `apply_raw` calls only `dup2`, which is
                // async-signal-safe and allocates nothing.
                if self.redirections.apply_raw().is_err() {
                    const FAILED: &[u8] = b"rsh: cannot redirect\n";

                    // SAFETY: `write(2)` is async-signal-safe and the buffer is
                    // a 'static byte string.
                    let _ = unsafe { libc::write(2, FAILED.as_ptr().cast(), FAILED.len()) };

                    // SAFETY: `_exit` is async-signal-safe and terminates
                    // without running handlers or flushing inherited buffers.
                    unsafe { libc::_exit(EXIT_REDIRECT_FAILED) }
                }

                // Put SIGPIPE back to its default action.
                //
                // Rust's runtime sets SIGPIPE to SIG_IGN at startup so that a
                // Rust program gets an `EPIPE` error instead of dying. That is
                // a fine default for a Rust program and a disastrous one for a
                // shell: SIG_IGN is *inherited across exec* — only installed
                // handlers are reset — so every child of `rsh` would start with
                // SIGPIPE ignored.
                //
                // The visible consequence is `yes | head -1`. With the default
                // action, `head` exits, `yes` is killed by SIGPIPE, and the
                // pipeline ends. With SIG_IGN inherited, `yes` gets a write
                // error it was never written to expect, and how long it spins
                // depends on how carefully somebody handled `EPIPE`.
                //
                // See experiments/pipes.
                //
                // SAFETY: `signal` is async-signal-safe and this sets a
                // disposition rather than installing a handler, so nothing here
                // can run Rust code in a signal context.
                unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };

                // SAFETY: `path` and `argv_ptrs` were built before the fork and
                // are still alive in this address-space copy. `argv_ptrs` is
                // NULL-terminated, which is `execv`'s only requirement beyond
                // validity. On success this call does not return.
                unsafe { libc::execv(self.path.as_ptr(), self.argv_ptrs.as_ptr()) };

                // `execv` only returns on failure. The path was already checked
                // for existence and execute permission before the fork, so
                // reaching here means something the parent could not see:
                // ENOEXEC on a malformed binary, ETXTBSY, a missing loader.
                const FAILED: &[u8] = b"rsh: exec failed\n";

                // SAFETY: `write(2)` is async-signal-safe. The buffer is a
                // 'static byte string, valid for the whole program.
                let _ = unsafe { libc::write(2, FAILED.as_ptr().cast(), FAILED.len()) };

                // SAFETY: `_exit` is async-signal-safe and terminates
                // immediately. `exit` would not do: it runs `atexit` handlers
                // and flushes the stdio buffers this child inherited from the
                // parent, duplicating output the parent has queued but not yet
                // written.
                unsafe { libc::_exit(EXIT_NOT_EXECUTABLE) }
            }

            Err(errno) => Err(SpawnError::Fork(errno)),
        }
    }
}

/// How a child stopped being runnable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// It ended.
    Finished(ExitStatus),
    /// It was suspended and is still there.
    Stopped(Signal),
}

/// A running child process.
#[derive(Debug)]
pub struct Child {
    pid: Pid,
}

impl Child {
    /// The child's process id.
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Block until the child terminates, then reap it.
    ///
    /// Consumes the handle: a pid is only meaningful until it is reaped, after
    /// which the kernel is free to reuse the number. Taking `self` by value
    /// makes "waiting twice" — and with it a whole family of bugs where a
    /// shell signals a pid that now belongs to something else — a compile
    /// error rather than a race.
    pub fn wait(self) -> Result<ExitStatus, SpawnError> {
        self.wait_with(|_| {})
    }

    /// Wait for the child to end *or* stop, whichever comes first.
    ///
    /// Takes `&self` rather than consuming: a stopped child is still a child,
    /// and the shell may well wait for it again after resuming it. Only the
    /// consuming [`Child::wait`] promises the process has been reaped.
    ///
    /// This is the call a shell with job control wants. [`Child::wait`] hides
    /// stops by continuing through them, which is right only when there is
    /// nowhere to put a suspended job.
    pub fn wait_or_stop(&self) -> Result<Waited, SpawnError> {
        loop {
            match waitpid(self.pid, Some(WaitPidFlag::WUNTRACED)) {
                Ok(WaitStatus::Stopped(_, signal)) => return Ok(Waited::Stopped(signal)),
                Ok(status) => match ExitStatus::from_wait(status) {
                    Some(status) => return Ok(Waited::Finished(status)),
                    None => continue,
                },
                Err(Errno::EINTR) => continue,
                Err(errno) => return Err(SpawnError::Wait(errno)),
            }
        }
    }

    /// Resume a stopped child.
    pub fn resume(&self) -> Result<(), SpawnError> {
        kill(self.pid, Signal::SIGCONT).map_err(SpawnError::Wait)
    }

    /// Block until the child terminates, reporting any stop along the way.
    ///
    /// `WUNTRACED` is what makes a stopped child visible at all. Without it,
    /// `waitpid` simply does not return for a process that has been suspended —
    /// so Ctrl-Z on a foreground command leaves the shell blocked forever on a
    /// child that will never finish, with no prompt and no way back.
    ///
    /// Having seen the stop, the shell has exactly two options: keep the job
    /// somewhere and hand the terminal back to itself, or continue the child
    /// and carry on waiting. The first is job control and needs a job table,
    /// process groups, and terminal ownership — Phase 6 and Phase 7. Until
    /// then this does the second, because the alternative is stranding a
    /// stopped process with nothing able to resume it.
    pub fn wait_with<F>(self, mut on_stopped: F) -> Result<ExitStatus, SpawnError>
    where
        F: FnMut(Signal),
    {
        loop {
            match waitpid(self.pid, Some(WaitPidFlag::WUNTRACED)) {
                Ok(WaitStatus::Stopped(_, signal)) => {
                    on_stopped(signal);
                    kill(self.pid, Signal::SIGCONT).map_err(SpawnError::Wait)?;
                }
                Ok(status) => match ExitStatus::from_wait(status) {
                    Some(status) => return Ok(status),
                    // Some other non-terminal event. The child is still alive,
                    // so keep waiting for its real end.
                    None => continue,
                },
                // A signal arrived while we were blocked. In an interactive
                // shell that is routine — it is what Ctrl-C looks like from
                // here — and it is not an error.
                Err(Errno::EINTR) => continue,
                Err(errno) => return Err(SpawnError::Wait(errno)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::Signal;
    use std::path::PathBuf;

    /// `/bin/sh` is guaranteed by POSIX to exist at that path.
    fn sh() -> PathBuf {
        PathBuf::from("/bin/sh")
    }

    fn run_sh(script: &str) -> ExitStatus {
        Command::new(&sh(), ["sh", "-c", script])
            .expect("failed to prepare command")
            .spawn()
            .expect("failed to fork")
            .wait()
            .expect("failed to wait")
    }

    // Note: the test harness is multi-threaded, so these tests fork from a
    // process with several live threads — precisely the situation the child
    // path is written to survive.

    #[test]
    fn a_child_that_exits_zero_reports_success() {
        assert_eq!(run_sh("exit 0"), ExitStatus::Exited(0));
    }

    #[test]
    fn a_child_exit_code_is_preserved() {
        assert_eq!(run_sh("exit 7"), ExitStatus::Exited(7));
    }

    #[test]
    fn a_child_killed_by_a_signal_is_reported_as_signaled() {
        let status = run_sh("kill -TERM $$");
        assert_eq!(status, ExitStatus::Signaled(Signal::SIGTERM));
        assert_eq!(status.code(), 143);
    }

    #[test]
    fn arguments_reach_the_program() {
        // `[ ... ]` exits 0 only if the arguments arrived intact and in order.
        assert_eq!(run_sh(r#"[ "$0" = sh ] || exit 1"#), ExitStatus::Exited(0));
        let status = Command::new(&sh(), ["sh", "-c", r#"[ "$1" = "a b" ]"#, "sh", "a b"])
            .unwrap()
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(status, ExitStatus::Exited(0));
    }

    #[test]
    fn exec_failure_in_the_child_is_reported_as_not_executable() {
        // A file that exists and is readable but is not a valid program: exec
        // fails with ENOEXEC *after* the fork, which is the case the child's
        // fallback path exists for.
        let status = Command::new(Path::new("/etc/hosts"), ["hosts"])
            .unwrap()
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
        assert_eq!(status, ExitStatus::Exited(EXIT_NOT_EXECUTABLE));
    }

    #[test]
    fn interior_nul_is_rejected_before_forking() {
        let err = Command::new(&sh(), ["sh", "-c", "ex\0it"]).unwrap_err();
        assert!(matches!(err, SpawnError::InteriorNul { .. }));
    }
}
