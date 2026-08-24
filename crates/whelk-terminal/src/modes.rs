//! Terminal modes: what the driver does with keystrokes before a program sees
//! them.
//!
//! In **canonical** mode — the default — the terminal driver buffers a line,
//! handles backspace and Ctrl-U itself, echoes what is typed, and turns Ctrl-C
//! into a signal. A program's `read` returns only when Enter is pressed. This
//! is why a shell gets line editing for free before it writes any, and why
//! Ctrl-C works before anyone installs a handler.
//!
//! In **raw** mode, none of that happens. Every keystroke arrives immediately,
//! nothing is echoed, and Ctrl-C is a byte (`0x03`) rather than a signal.
//! Editors and full-screen programs need this; so does any shell that wants to
//! implement its own history and arrow keys, which is Phase 8.
//!
//! # Why the shell saves them
//!
//! Modes belong to the terminal, not to the process that set them. `vim` puts
//! the terminal in raw mode; if it is killed, nothing puts it back. The symptom
//! is familiar — a terminal that stops echoing what you type — and the usual
//! remedy is to type `reset` blind.
//!
//! A shell can do better, because it is still running: it snapshots the modes
//! at startup and restores them after every foreground job. The job did not
//! have to cooperate, and did not have to survive.

use nix::errno::Errno;
use nix::sys::termios::{
    tcgetattr, tcsetattr, LocalFlags, SetArg, SpecialCharacterIndices, Termios,
};

use crate::ownership::terminal;

/// A snapshot of a terminal's settings.
///
/// Opaque on purpose: the only useful operations are taking one and putting it
/// back. A caller that wanted to inspect the flags would be reimplementing the
/// part of this crate that knows what they mean.
#[derive(Debug, Clone)]
pub struct Modes(Termios);

/// Capture the terminal's current settings.
///
/// Returns `None` when there is no terminal — a script, a pipe, a CI runner.
/// That is not an error; it is a shell with nothing to restore.
pub fn snapshot() -> Option<Modes> {
    tcgetattr(terminal()).ok().map(Modes)
}

/// Put previously captured settings back.
///
/// `TCSADRAIN` waits for pending output to be written first. Changing modes out
/// from under bytes still in the driver's queue is how output ends up mangled
/// at exactly the moment a program exits — the last thing anyone wants to debug.
pub fn restore(modes: &Modes) -> Result<(), Errno> {
    tcsetattr(terminal(), SetArg::TCSADRAIN, &modes.0)
}

/// Put the terminal into raw mode until the guard is dropped.
///
/// Not used by the REPL yet — line editing is Phase 8 — but the capability is
/// what makes that phase a change to the input loop rather than a change to
/// this crate. It is exercised by tests through a pseudoterminal.
pub fn raw() -> Result<RawGuard, Errno> {
    let original = tcgetattr(terminal())?;
    let mut raw = original.clone();

    // ICANON off: reads return immediately rather than waiting for Enter.
    // ECHO off: the driver stops printing keystrokes, because a line editor
    //   needs to decide for itself what appears where.
    // ISIG off: Ctrl-C arrives as the byte 0x03 instead of becoming SIGINT.
    // IEXTEN off: Ctrl-V stops being "quote the next character".
    raw.local_flags
        .remove(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG | LocalFlags::IEXTEN);

    // A read returns as soon as one byte is available, and never times out.
    raw.control_chars[SpecialCharacterIndices::VMIN as usize] = 1;
    raw.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;

    tcsetattr(terminal(), SetArg::TCSADRAIN, &raw)?;
    Ok(RawGuard {
        original: Modes(original),
    })
}

/// Restores the terminal's previous modes when dropped.
///
/// A guard rather than a matching call, for the same reason redirection uses
/// one: the restore has to happen on every path out, including a panic, and
/// `Drop` is the only construct that promises that. A shell that returned early
/// from an error while the terminal was raw would leave the user with no echo
/// and no working Ctrl-C.
#[derive(Debug)]
pub struct RawGuard {
    original: Modes,
}

impl RawGuard {
    /// The settings that will be restored.
    pub fn original(&self) -> &Modes {
        &self.original
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        // Nothing useful to do on failure: the terminal is already lost, and
        // there is nowhere left to report it.
        let _ = restore(&self.original);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_nothing_to_snapshot_without_a_terminal() {
        // The test harness has no terminal, which is the case the shell must
        // handle without complaining: a script has no modes to preserve.
        if crate::ownership::is_interactive() {
            assert!(snapshot().is_some());
        } else {
            assert!(snapshot().is_none());
        }
    }
}
