//! Rearranging file descriptors.
//!
//! A redirection is not an instruction to a program. `grep` has no idea it is
//! writing to a file; it writes to descriptor 1 exactly as it always does, and
//! the shell arranged in advance for descriptor 1 to *be* the file. The whole
//! mechanism is `dup2`, applied in the child between `fork` and `exec`, and the
//! program never learns it happened.
//!
//! ```text
//!            before dup2                        after dup2(7, 1)
//!    ┌────┐                                ┌────┐
//!  0 │    │──► terminal                  0 │    │──► terminal
//!  1 │    │──► terminal                  1 │    │──┐
//!  2 │    │──► terminal                  2 │    │──┼► terminal
//!  7 │    │──► out.txt                   7 │    │──┼► out.txt
//!    └────┘                                └────┘  └──────────┘
//!                                                  1 and 7 now share
//!                                                  one open file
//! ```
//!
//! # Where the work happens
//!
//! Opening the file happens in the parent, before the fork. That is the same
//! rule as `PATH` resolution in [`crate::path`], for the same reason: a missing
//! file or a permission error is an ordinary `Result` in the parent, and an
//! unreportable disaster in the child. See `docs/process-model.md`.
//!
//! What is left for the child is `dup2` and nothing else — one syscall per
//! redirection, async-signal-safe, no allocation.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use nix::errno::Errno;
use nix::fcntl::{fcntl, FcntlArg};
use nix::libc;

/// One descriptor change: make `target` refer to whatever `source` refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Action {
    target: RawFd,
    source: RawFd,
}

/// A set of descriptor changes, ready to apply.
///
/// Order is preserved, and it matters. `>out 2>&1` sends both streams to the
/// file, while `2>&1 >out` sends stderr to wherever stdout pointed *before*
/// the file was opened — a difference that has confused people for forty years
/// and is entirely explained by these actions running left to right.
#[derive(Debug, Default)]
pub struct Redirections {
    /// Files opened by the parent, kept alive so the raw descriptors recorded
    /// in `actions` stay valid until the child has used them.
    ///
    /// These are all close-on-exec: `dup2` clears that flag on the *copy*, so
    /// the descriptor the program inherits survives while this original closes
    /// itself automatically. The program gets exactly the descriptors it should
    /// have, and no accidental extras.
    files: Vec<OwnedFd>,
    actions: Vec<Action>,
}

impl Redirections {
    /// An empty set — no descriptors change.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there is anything to do.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Point `target` at an already-open file.
    ///
    /// Takes ownership because the descriptor must outlive the fork; dropping
    /// it early would close the file before the child could use it.
    pub fn redirect_to_file(&mut self, target: RawFd, file: OwnedFd) {
        self.actions.push(Action {
            target,
            source: file.as_raw_fd(),
        });
        self.files.push(file);
    }

    /// Point `target` at whatever `source` currently refers to — `2>&1`.
    pub fn duplicate(&mut self, target: RawFd, source: RawFd) {
        self.actions.push(Action { target, source });
    }

    /// Whether a descriptor is open in this process.
    ///
    /// Checked in the parent so that `2>&9` fails with a message instead of
    /// failing inside the child, where the only way to report it is an exit
    /// code.
    pub fn is_open(fd: RawFd) -> bool {
        fcntl(unsafe_borrow(fd), FcntlArg::F_GETFD).is_ok()
    }

    /// Apply every action to the calling process.
    ///
    /// # Safety
    ///
    /// Only safe to call between `fork` and `exec`, or through
    /// [`Redirections::apply_scoped`], which arranges to put things back.
    /// Everything this function calls — `dup2` alone — is async-signal-safe and
    /// allocates nothing, which is what makes it usable in the child.
    ///
    /// Returns the descriptor that could not be redirected, if any. It does not
    /// return an `Err` carrying an [`Errno`], because formatting one allocates
    /// and the child must not.
    pub(crate) fn apply_raw(&self) -> Result<(), RawFd> {
        for action in &self.actions {
            // `dup2(fd, fd)` is defined to do nothing at all — including *not*
            // clearing FD_CLOEXEC, which is normally the useful side effect.
            //
            // This is not a hypothetical. `sh -c '...' 3> f` opens the file at
            // the lowest free descriptor, which is 3, and then asks to put it
            // on descriptor 3. Skipping the call would leave the close-on-exec
            // flag set and the program would start with no descriptor 3 at
            // all — the redirection would silently do nothing.
            if action.source == action.target {
                // SAFETY: `fcntl` is async-signal-safe. Clearing the flags word
                // to zero clears FD_CLOEXEC, which is the only flag defined for
                // it, so this cannot disturb anything else.
                if unsafe { libc::fcntl(action.target, libc::F_SETFD, 0) } < 0 {
                    return Err(action.target);
                }
                continue;
            }

            // SAFETY: `dup2` is async-signal-safe. Both descriptors are plain
            // integers captured before the fork; if either is stale the call
            // fails with EBADF rather than doing something undefined.
            if unsafe { libc::dup2(action.source, action.target) } < 0 {
                return Err(action.target);
            }
        }

        Ok(())
    }

    /// Apply every action to the *current* process, restoring on drop.
    ///
    /// This is how a builtin gets redirected. `cd` runs inside the shell, so
    /// there is no child to arrange descriptors for — the shell has to move its
    /// own, then put them back, or every later command would inherit the
    /// change.
    pub fn apply_scoped(&self) -> Result<Restore, Errno> {
        let mut restore = Restore {
            saved: Vec::with_capacity(self.actions.len()),
        };

        for action in &self.actions {
            restore.saved.push((action.target, save(action.target)?));

            if action.source != action.target {
                // SAFETY: both descriptors are valid for the duration of this
                // call; the originals are held open by `self.files` or by the
                // process itself.
                if unsafe { libc::dup2(action.source, action.target) } < 0 {
                    // `restore` is dropped here, undoing whatever succeeded.
                    return Err(Errno::last());
                }
            }
        }

        Ok(restore)
    }
}

/// A descriptor's previous state, to be put back.
#[derive(Debug)]
enum Saved {
    /// It was open, and here is a copy of it.
    Was(OwnedFd),
    /// It was closed, so restoring means closing it again.
    WasClosed,
}

/// Copy a descriptor so it can be put back later.
///
/// The copy is placed at 10 or above and marked close-on-exec, so it neither
/// collides with the descriptors a user might redirect nor leaks into any child
/// spawned while it is held.
fn save(fd: RawFd) -> Result<Saved, Errno> {
    match fcntl(unsafe_borrow(fd), FcntlArg::F_DUPFD_CLOEXEC(10)) {
        // SAFETY: `fcntl` just created this descriptor and returned the only
        // handle to it, so taking ownership here cannot double-close.
        Ok(copy) => Ok(Saved::Was(unsafe { OwnedFd::from_raw_fd(copy) })),
        // Redirecting a descriptor that was not open is legal — `3>file` on a
        // shell with no descriptor 3. Putting it back means closing it.
        Err(Errno::EBADF) => Ok(Saved::WasClosed),
        Err(errno) => Err(errno),
    }
}

/// Puts the shell's descriptors back when it goes out of scope.
///
/// A guard rather than a matching call, because the alternative is a shell that
/// loses its own stdout the first time a builtin returns early. Restoration has
/// to happen on every path out, and `Drop` is the only construct that promises
/// that.
#[derive(Debug)]
pub struct Restore {
    saved: Vec<(RawFd, Saved)>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        // Reverse order, so a descriptor redirected twice ends up where it
        // started rather than at its intermediate value.
        for (target, saved) in self.saved.drain(..).rev() {
            match saved {
                Saved::Was(copy) => {
                    // SAFETY: `copy` is a live descriptor owned by this struct
                    // and `target` is a plain integer. A failure here would
                    // mean the shell's own descriptors are already lost, and
                    // there would be nowhere left to report it — hence the
                    // deliberately ignored result.
                    let _ = unsafe { libc::dup2(copy.as_raw_fd(), target) };
                }
                Saved::WasClosed => {
                    // SAFETY: closing a descriptor this process opened, to put
                    // it back in the state the redirection found it in.
                    let _ = unsafe { libc::close(target) };
                }
            }
        }
    }
}

/// Borrow a raw descriptor for the duration of a single `fcntl` call.
///
/// `fcntl` needs a `BorrowedFd`, and the descriptors here are raw integers that
/// this module does not own. The borrow never outlives the call.
fn unsafe_borrow(fd: RawFd) -> std::os::fd::BorrowedFd<'static> {
    // SAFETY: the returned borrow is used only within the calling expression,
    // and the callers below pass descriptors that are either open in this
    // process or invalid — in which case `fcntl` reports EBADF, which is
    // exactly the answer being asked for.
    unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }
}
