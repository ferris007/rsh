//! `rsh` — a Unix shell built from first principles.
//!
//! This binary is only the read-eval-print loop: read a line, hand it to the
//! executor, decide whether to loop again. Everything that touches the process
//! table lives in `rsh-process`; everything that decides what a line means
//! lives in `rsh-parser` and `rsh-executor`.

use std::io::{self, BufRead, IsTerminal, Write};
use std::process::ExitCode;

use rsh_executor::{Outcome, Shell};

/// The prompt. Phase 8 makes this configurable; for now it is a constant so
/// that nothing about the prompt can affect what a command does.
const PROMPT: &str = "rsh> ";

fn main() -> ExitCode {
    let stdin = io::stdin();

    // Interactivity is decided by stdin alone. A shell reading a script from a
    // pipe should print no prompts, but it is still the same shell — the
    // difference is cosmetic, not behavioural, and nothing below this line
    // branches on it.
    let interactive = stdin.is_terminal();

    let mut input = stdin.lock();
    let mut shell = Shell::new();
    let mut line = String::new();

    let status = loop {
        if interactive {
            prompt();
        }

        line.clear();
        match input.read_line(&mut line) {
            // End of input. Ctrl-D at a prompt means "no more commands", not
            // "something went wrong", so the shell leaves with the status of
            // the last command it ran — the same status `exit` would use.
            Ok(0) => {
                if interactive {
                    // The user's Ctrl-D left the cursor after the prompt.
                    eprintln!();
                }
                break shell.last_status();
            }
            Ok(_) => match shell.run_line(&line) {
                Outcome::Continue => {}
                Outcome::Exit(status) => break status,
            },
            Err(error) => {
                eprintln!("rsh: failed to read input: {error}");
                break 1;
            }
        }
    };

    // Statuses are a single byte to the operating system: `exit 300` becomes
    // 44 no matter what any shell does, so truncate here rather than pretend
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
