//! How big the terminal is.
//!
//! The size is not a property a program is told once. A window can be resized
//! at any moment, and the kernel announces it with `SIGWINCH` to the foreground
//! process group — so a program that cached the answer at startup is wrong the
//! first time someone drags a corner.
//!
//! A shell has a specific reason to care even without a full-screen interface:
//! `COLUMNS` and `LINES` are environment variables that children read. A shell
//! that never updates them hands every program it runs a stale idea of the
//! window, and the symptom is `less` or `ps` formatting for the wrong width.

use nix::libc::{ioctl, winsize, TIOCGWINSZ};
use std::os::fd::AsRawFd;

use crate::ownership::terminal;

/// A terminal's dimensions, in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// Rows, which shells call `LINES`.
    pub rows: u16,
    /// Columns, which shells call `COLUMNS`.
    pub cols: u16,
}

/// Ask the terminal how big it is.
///
/// Returns `None` when there is no terminal, or when it has no size to report —
/// which is normal for a pty that nobody has sized.
pub fn size() -> Option<Size> {
    let mut ws = winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // SAFETY: `TIOCGWINSZ` writes a `winsize` through the pointer, and that is
    // exactly what is passed. A bad descriptor returns -1 rather than writing.
    let result = unsafe { ioctl(terminal().as_raw_fd(), TIOCGWINSZ, &raw mut ws) };

    if result < 0 || ws.ws_row == 0 || ws.ws_col == 0 {
        return None;
    }

    Some(Size {
        rows: ws.ws_row,
        cols: ws.ws_col,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipe_has_no_size() {
        if !crate::ownership::is_interactive() {
            assert_eq!(size(), None);
        }
    }
}
