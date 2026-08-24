//! Reading lines, interruptibly.
//!
//! The obvious implementation — `BufRead::read_line` on stdin — cannot express
//! Ctrl-C. Rust's buffered readers treat `EINTR` as "try again" and loop
//! internally, which is correct for a program that has no opinion about signals
//! and useless for a shell, whose entire response to Ctrl-C is to abandon the
//! line it was reading.
//!
//! So this reads the descriptor directly and lets `EINTR` through. That also
//! means owning the line buffering, because a `read` on a pipe returns whatever
//! is available — which may be several lines, or half of one.

use std::fs::File;
use std::io::{self, ErrorKind, Read};
use std::mem::ManuallyDrop;
use std::os::fd::FromRawFd;

/// What a read produced.
///
/// Shared with the interactive reader in [`crate::interactive`], so the REPL
/// handles a line the same way whether it was typed or piped in.
#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    /// A line, without its trailing newline.
    Line(String),
    /// A signal arrived; the partial line, if any, has been discarded.
    Interrupted,
    /// End of input — Ctrl-D, or the end of a script.
    EndOfFile,
}

/// A line-oriented reader over a raw descriptor.
#[derive(Debug)]
pub struct Reader {
    /// Bytes read but not yet returned as a line.
    pending: Vec<u8>,
    /// Whether the descriptor has reported end of input.
    finished: bool,
}

impl Reader {
    /// A reader over the shell's standard input.
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            finished: false,
        }
    }

    /// Read the next line.
    pub fn next_line(&mut self) -> io::Result<Input> {
        loop {
            if let Some(line) = self.take_line() {
                return Ok(Input::Line(line));
            }

            if self.finished {
                // A last line with no trailing newline is still a line.
                return Ok(if self.pending.is_empty() {
                    Input::EndOfFile
                } else {
                    Input::Line(decode(std::mem::take(&mut self.pending)))
                });
            }

            let mut chunk = [0_u8; 1024];

            // A `File` over descriptor 0 rather than `io::stdin()`, because
            // `File::read` is a bare `read(2)` — it reports `EINTR` instead of
            // quietly retrying, which is the whole point of this module.
            //
            // SAFETY: descriptor 0 is standard input and outlives this call.
            // `ManuallyDrop` keeps the `File` from closing it on the way out,
            // which would leave the shell without an input descriptor.
            let mut stdin = ManuallyDrop::new(unsafe { File::from_raw_fd(0) });

            match stdin.read(&mut chunk) {
                Ok(0) => self.finished = true,
                Ok(count) => self.pending.extend_from_slice(&chunk[..count]),

                // The signal handler ran. Whatever the user had typed is
                // abandoned — which is what Ctrl-C means — and the caller
                // decides what to report.
                Err(error) if error.kind() == ErrorKind::Interrupted => {
                    self.pending.clear();
                    return Ok(Input::Interrupted);
                }

                Err(error) => return Err(error),
            }
        }
    }

    /// Split off a complete line, if one is buffered.
    fn take_line(&mut self) -> Option<String> {
        let newline = self.pending.iter().position(|&byte| byte == b'\n')?;
        let mut line: Vec<u8> = self.pending.drain(..=newline).collect();
        line.pop();
        Some(decode(line))
    }
}

/// Turn bytes into a line.
///
/// Lossy on purpose. A filename in some other encoding is a bad argument, not a
/// reason for the shell to stop reading input.
fn decode(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed a reader from a buffer rather than a descriptor, to test the line
    /// splitting without touching standard input.
    fn lines_of(input: &str) -> Vec<Input> {
        let mut reader = Reader {
            pending: input.as_bytes().to_vec(),
            finished: true,
        };
        let mut out = Vec::new();
        loop {
            match reader.next_line().expect("reading a buffer cannot fail") {
                Input::EndOfFile => {
                    out.push(Input::EndOfFile);
                    return out;
                }
                other => out.push(other),
            }
        }
    }

    #[test]
    fn splits_on_newlines() {
        assert_eq!(
            lines_of("one\ntwo\n"),
            [
                Input::Line("one".into()),
                Input::Line("two".into()),
                Input::EndOfFile
            ]
        );
    }

    #[test]
    fn a_final_line_without_a_newline_is_still_a_line() {
        assert_eq!(
            lines_of("only"),
            [Input::Line("only".into()), Input::EndOfFile]
        );
    }

    #[test]
    fn empty_lines_are_preserved() {
        // The shell has to see them: a blank line is not end of input.
        assert_eq!(
            lines_of("\n\n"),
            [
                Input::Line(String::new()),
                Input::Line(String::new()),
                Input::EndOfFile
            ]
        );
    }

    #[test]
    fn no_input_is_end_of_file() {
        assert_eq!(lines_of(""), [Input::EndOfFile]);
    }

    #[test]
    fn invalid_utf8_does_not_stop_the_shell() {
        let mut reader = Reader {
            pending: vec![b'a', 0xff, b'b', b'\n'],
            finished: true,
        };
        let Input::Line(line) = reader.next_line().unwrap() else {
            panic!("expected a line")
        };
        assert!(line.starts_with('a') && line.ends_with('b'));
    }
}
