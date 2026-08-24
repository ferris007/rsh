//! Reading a line from a terminal.
//!
//! The non-interactive path in [`crate::input`] reads bytes and splits on
//! newlines, which is all a script needs. This is the other one: raw mode, a
//! line editor, history, and completion.
//!
//! # Waiting for more than one thing
//!
//! The read is driven by a poller rather than by blocking on descriptor 0.
//! Standard input and the signal self-pipe are both registered, so the loop
//! wakes for either — which is what lets a window resize redraw the line while
//! the user is still typing it, and lets a finished background job be reaped
//! promptly rather than at the next prompt.
//!
//! Standard input is deliberately **not** put into non-blocking mode. `O_NONBLOCK`
//! belongs to the open file description, not to the descriptor, so setting it
//! on descriptor 0 sets it for every process that shares that description —
//! including every child the shell starts. A `cat` that suddenly gets `EAGAIN`
//! from a terminal is a bug the shell caused and the user cannot explain.
//!
//! The poller makes that unnecessary: a read that happens only after readiness
//! is reported returns immediately whether or not the descriptor is blocking.
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
use std::os::fd::{FromRawFd, RawFd};
use std::path::PathBuf;

use rsh_event::{Poller, Token};
use rsh_executor::Shell;
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
    poller: Poller,
}

/// The signal self-pipe.
const SIGNALS: Token = Token(1);

/// Standard input.
const INPUT: Token = Token(2);

impl Interactive {
    /// Start a session, loading history from the user's home directory.
    pub fn new(signal_fd: Option<RawFd>) -> Self {
        let history_path =
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(HISTORY_FILE));

        let history = match &history_path {
            Some(path) => History::load(path, HISTORY_LIMIT),
            None => History::new(HISTORY_LIMIT),
        };

        let mut poller = Poller::new().expect("failed to create a poller");
        poller
            .watch(0, INPUT)
            .expect("failed to watch standard input");

        // Registered once, for the life of the shell. A signal arriving while
        // the loop is blocked writes a byte here, and the poller returns.
        if let Some(fd) = signal_fd {
            poller
                .watch(fd, SIGNALS)
                .expect("failed to watch the signal pipe");
        }

        Self {
            editor: Editor::new(history),
            completer: complete::Shell,
            history_path,
            poller,
        }
    }

    /// Write history back out.
    pub fn save_history(&self) {
        if let Some(path) = &self.history_path {
            self.editor.history().save(path);
        }
    }

    /// React to whatever signal woke the loop.
    ///
    /// Returns `Some` when the line should be abandoned; `None` means the
    /// signal was handled and editing continues.
    fn signals(&mut self, prompt: &str, shell: &mut Shell) -> std::io::Result<Option<Input>> {
        if shell.shutdown_requested().is_some() {
            return Ok(Some(Input::EndOfFile));
        }

        // A resize can be acted on immediately now, rather than at the next
        // prompt: the line is redrawn at its new width while the user is still
        // typing it. This is the first thing the event loop makes possible that
        // a blocking read could not.
        // Whoever takes the flag has to handle it completely: the main loop
        // takes it when a resize happens while a command runs, and this takes
        // it when one happens mid-line. A consumer that only did half the work
        // would leave COLUMNS stale depending on when the window was dragged.
        if shell.take_resize() {
            shell.refresh_window_size(true);

            let mut out = std::io::stderr();
            write!(out, "{}", rsh_line::render_line(prompt, &self.editor))?;
            out.flush()?;
        }

        // Child events are left for the main loop to collect and report.
        // Printing them here would write over the line being edited.
        Ok(None)
    }

    /// Read one line, editing it as the user types.
    pub fn read_line(&mut self, prompt: &str, shell: &mut Shell) -> std::io::Result<Input> {
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
            let ready = match self.poller.wait(None) {
                Ok(events) => events.to_vec(),
                // A signal arrived during the wait. The self-pipe has a byte in
                // it, so the next pass will see it; nothing to do here.
                Err(rsh_event::INTERRUPTED) => continue,
                Err(error) => return Err(std::io::Error::from_raw_os_error(error as i32)),
            };

            let mut input_ready = false;
            for event in &ready {
                match event.token {
                    SIGNALS => {
                        shell.drain_signal_notifications();
                        if let Some(abandon) = self.signals(prompt, shell)? {
                            return Ok(abandon);
                        }
                    }
                    _ => input_ready = true,
                }
            }

            if !input_ready {
                continue;
            }

            // SAFETY: descriptor 0 is standard input and outlives this call.
            // `ManuallyDrop` keeps the `File` from closing it on the way out.
            let mut stdin = ManuallyDrop::new(unsafe { File::from_raw_fd(0) });

            let count = match stdin.read(&mut chunk) {
                Ok(0) => {
                    // The terminal went away mid-line.
                    return Ok(Input::EndOfFile);
                }
                Ok(count) => count,
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
