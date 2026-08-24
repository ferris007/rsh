//! Reading a line from a terminal.
//!
//! The non-interactive path in [`crate::input`] reads bytes and splits on
//! newlines, which is all a script needs. This is the other one: raw mode, a
//! line editor, history, and completion.
//!
//! # Raw mode is entered per line, not per session
//!
//! The terminal is put into raw mode when a line starts and restored when it is
//! submitted, so a command runs with the terminal in the state it expects.
//! Holding raw mode across a command would hand every child a terminal that
//! does not echo and does not turn Ctrl-C into a signal — the shell's line
//! editor imposed on programs that never asked for it.
//!
//! The guard makes that automatic: it is dropped when the function returns, on
//! every path including a panic.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;
use std::path::PathBuf;

use rsh_line::{Action, Decoded, Editor, History};

use crate::complete;
use crate::input::Input;

/// How many commands to remember.
const HISTORY_LIMIT: usize = 5_000;

/// Where history is kept between sessions.
const HISTORY_FILE: &str = ".rsh_history";

/// An interactive reader: raw mode, editing, history, completion.
pub struct Interactive {
    editor: Editor,
    completer: complete::Shell,
    history_path: Option<PathBuf>,
}

impl Interactive {
    /// Start a session, loading history from the user's home directory.
    pub fn new() -> Self {
        let history_path =
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(HISTORY_FILE));

        let history = match &history_path {
            Some(path) => History::load(path, HISTORY_LIMIT),
            None => History::new(HISTORY_LIMIT),
        };

        Self {
            editor: Editor::new(history),
            completer: complete::Shell,
            history_path,
        }
    }

    /// Write history back out.
    pub fn save_history(&self) {
        if let Some(path) = &self.history_path {
            self.editor.history().save(path);
        }
    }

    /// Read one line, editing it as the user types.
    pub fn read_line(&mut self, prompt: &str) -> std::io::Result<Input> {
        // Raw mode for the duration of this line only. See the module note.
        let _raw = match rsh_terminal::raw() {
            Ok(guard) => guard,
            // No terminal, or it refused. Falling back to a plain prompt is
            // better than refusing to read a line at all.
            Err(error) => return Err(std::io::Error::from_raw_os_error(error as i32)),
        };

        self.editor.reset();
        let mut out = std::io::stderr();
        write!(out, "{}", rsh_line::render_line(prompt, &self.editor))?;
        out.flush()?;

        let mut pending: Vec<u8> = Vec::new();
        let mut chunk = [0_u8; 1024];

        loop {
            // SAFETY: descriptor 0 is standard input and outlives this call.
            // `ManuallyDrop` keeps the `File` from closing it on the way out.
            let mut stdin = ManuallyDrop::new(unsafe { File::from_raw_fd(0) });

            let count = match stdin.read(&mut chunk) {
                Ok(0) => {
                    // The terminal went away mid-line.
                    return Ok(Input::EndOfFile);
                }
                Ok(count) => count,
                // A signal arrived — a resize, or a background job finishing.
                // Neither is a reason to abandon the line.
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };

            pending.extend_from_slice(&chunk[..count]);

            while let Decoded::Key(key, used) = rsh_line::decode(&pending) {
                pending.drain(..used);

                match self.editor.handle(key, &self.completer) {
                    Action::Continue => {
                        write!(out, "{}", rsh_line::render_line(prompt, &self.editor))?;
                    }

                    Action::Suggest(candidates) => {
                        write!(
                            out,
                            "{}",
                            rsh_line::render_suggestions(prompt, &self.editor, &candidates)
                        )?;
                    }

                    Action::Clear => {
                        write!(out, "{}", rsh_line::CLEAR_SCREEN)?;
                        write!(out, "{}", rsh_line::render_line(prompt, &self.editor))?;
                    }

                    Action::Submit(line) => {
                        // The newline has to be written here: in raw mode the
                        // terminal did not echo the Enter, so without it the
                        // command's output would start on the prompt's line.
                        write!(out, "\r\n")?;
                        out.flush()?;
                        self.editor.remember(&line);
                        return Ok(Input::Line(line));
                    }

                    Action::Interrupted => {
                        write!(out, "\r\n")?;
                        out.flush()?;
                        return Ok(Input::Interrupted);
                    }

                    Action::EndOfFile => {
                        write!(out, "\r\n")?;
                        out.flush()?;
                        return Ok(Input::EndOfFile);
                    }
                }

                out.flush()?;
            }
        }
    }
}
