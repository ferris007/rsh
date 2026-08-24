//! `rsh` — a Unix shell built from first principles.
//!
//! This binary is only the read-eval-print loop: read a line, hand it to the
//! executor, decide whether to loop again. Everything that touches the process
//! table lives in `rsh-process`; everything that decides what a line means
//! lives in `rsh-parser` and `rsh-executor`.

mod input;

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use rsh_executor::{Outcome, Shell};

use crate::input::{Input, Reader};

/// The prompt. Phase 8 makes this configurable; for now it is a constant so
/// that nothing about the prompt can affect what a command does.
const PROMPT: &str = "rsh> ";

/// Status for a command interrupted by Ctrl-C: `128 + SIGINT`.
const EXIT_INTERRUPTED: i32 = 130;

fn main() -> ExitCode {
    // Before anything else. A shell that has not installed its handlers is a
    // shell that Ctrl-C kills.
    let mut shell = Shell::new();
    if let Err(error) = shell.install_signal_handlers() {
        eprintln!("rsh: cannot install signal handlers: {error}");
        return ExitCode::from(1);
    }

    // Interactivity is decided by stdin alone. A shell reading a script from a
    // pipe should print no prompts, but it is still the same shell.
    let interactive = io::stdin().is_terminal();

    let mut reader = Reader::new();

    let status = loop {
        // Checked before blocking on input, so a shell told to stop does not
        // sit at a prompt waiting for a line it will never use.
        if let Some(signal) = shell.shutdown_requested() {
            if interactive {
                eprintln!();
            }
            break 128 + signal;
        }

        // Say what background jobs have been up to, before the prompt rather
        // than the instant it happens: a notification that arrived mid-keystroke
        // would write over whatever the user was typing.
        shell.report_jobs();
        shell.refresh_window_size();

        if interactive {
            prompt();
        }

        match reader.next_line() {
            // End of input. Ctrl-D at a prompt means "no more commands", not
            // "something went wrong", so the shell leaves with the status of
            // the last command it ran — the same status `exit` would use.
            Ok(Input::EndOfFile) => {
                if interactive {
                    // The user's Ctrl-D left the cursor after the prompt.
                    eprintln!();
                }

                // Ctrl-D with stopped jobs gets the same warning `exit` does:
                // they would be left suspended with nothing able to resume
                // them. The second Ctrl-D goes through.
                if !shell.confirm_exit() {
                    continue;
                }

                // A shutdown request and an end of input can arrive together —
                // a terminal hanging up does both. Being asked to terminate is
                // the more specific fact, so it wins: reporting the last
                // command's status here would lose the reason the shell
                // stopped.
                break match shell.shutdown_requested() {
                    Some(signal) => 128 + signal,
                    None => shell.last_status(),
                };
            }

            // Ctrl-C while waiting for a line. The partial line is gone, the
            // status records the interruption, and the shell prompts again —
            // it does not exit, and it does not run anything.
            Ok(Input::Interrupted) => {
                eprintln!();
                shell.set_last_status(EXIT_INTERRUPTED);
            }

            Ok(Input::Line(line)) => {
                // Any Ctrl-C that arrived while the line was being typed
                // belongs to the line that was abandoned, not to this one.
                shell.take_interrupt();

                match shell.run_line(&line) {
                    Outcome::Continue => {}
                    Outcome::Exit(status) => break status,
                }

                // A foreground command shares the shell's process group, so
                // Ctrl-C reached the shell too. The command has already died of
                // it; the flag would otherwise fire at the next prompt and look
                // like a keystroke the user never made.
                if shell.take_interrupt() && interactive {
                    eprintln!();
                }
            }

            Err(error) => {
                eprintln!("rsh: failed to read input: {error}");
                break 1;
            }
        }
    };

    // Whatever the last job did to the terminal, undo it. The shell is the only
    // thing left that knows what the settings were before.
    shell.restore_terminal();

    // Statuses are a single byte to the operating system: `exit 300` becomes 44
    // no matter what any shell does, so truncate here rather than pretend
    // otherwise. The `& 0xff` also keeps negative arguments from wrapping into
    // a nonsense `u8` cast.
    ExitCode::from((status & 0xff) as u8)
}

/// Write the prompt.
///
/// It goes to stderr, not stdout. That is what lets `rsh > out.txt` stay usable
/// interactively: the prompt reaches the terminal while command output goes to
/// the file. Stdout is the command's channel, and the shell should not be
/// writing to it.
fn prompt() {
    let mut stderr = io::stderr();
    let _ = write!(stderr, "{PROMPT}");
    // stderr is unbuffered, but flushing states the requirement rather than
    // relying on it: the prompt has to be visible before the read blocks.
    let _ = stderr.flush();
}
