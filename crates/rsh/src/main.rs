//! `rsh` — a Unix shell built from first principles.
//!
//! This binary is only the read-eval-print loop: read a line, hand it to the
//! executor, decide whether to loop again. Everything that touches the process
//! table lives in `rsh-process`; everything that decides what a line means
//! lives in `rsh-parser` and `rsh-executor`.

mod benchmark;
mod complete;
mod input;
mod interactive;

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use rsh_executor::{Outcome, Shell};

use crate::input::{Input, Reader};
use crate::interactive::Interactive;

/// The prompt. Phase 8 makes this configurable; for now it is a constant so
/// that nothing about the prompt can affect what a command does.
const PROMPT: &str = "rsh> ";

/// Status for a command interrupted by Ctrl-C: `128 + SIGINT`.
const EXIT_INTERRUPTED: i32 = 130;

/// Status for a command line the shell could not make sense of.
///
/// Two, because that is what a shell reports for its own usage errors, as
/// distinct from 127 for a command it could not find.
const EXIT_USAGE: i32 = 2;

/// What `--help` prints.
///
/// The list of what is missing is part of the help rather than a footnote. A
/// person who has just installed this and types `ls *.rs` deserves to find out
/// why it did not glob from the shell itself, not from an issue tracker.
const HELP: &str = "\
rsh - a Unix shell built from first principles.

usage:
  rsh                 start a shell; a terminal gets editing, history, and completion
  rsh < script        read commands from a file or a pipe, without prompts
  rsh --benchmark     measure startup, a command, and a pipeline
  rsh --help          print this
  rsh --version       print the version

Commands are read from standard input. There is no run-this-file form.

Deliberately not implemented: &&, ||, ;, globbing, command substitution,
here-documents, shell functions, aliases, and multi-line input. These are
parsed and refused by name rather than mistaken for arguments.
";

/// What the command line asked the shell to do.
///
/// Anything unrecognised is reported rather than ignored. A shell that quietly
/// drops an option it does not know has claimed to understand something it did
/// not, and the rest of this project refuses what it cannot do by name. The
/// command line is held to the same standard.
enum Request {
    /// Start the read-eval-print loop.
    Run,
    Help,
    Version,
    Benchmark,
    /// An option the shell does not have.
    UnknownOption(String),
    /// A bare word, which is almost always someone expecting `rsh script.sh`.
    NotAScriptRunner(String),
}

/// Read the request from the arguments, which are the ones after argv[0].
fn request(mut arguments: impl Iterator<Item = String>) -> Request {
    let Some(first) = arguments.next() else {
        return Request::Run;
    };

    match first.as_str() {
        "--help" | "-h" => Request::Help,
        "--version" | "-V" => Request::Version,
        "--benchmark" => Request::Benchmark,
        // A leading dash is a request for behaviour; anything else is a name,
        // and the two deserve different answers.
        other if other.starts_with('-') => Request::UnknownOption(other.to_owned()),
        other => Request::NotAScriptRunner(other.to_owned()),
    }
}

fn main() -> ExitCode {
    // The command line is settled before anything is set up. The benchmark
    // starts fresh shells of its own and a half-initialised one would measure
    // the wrong thing, and the rest of these answer and leave without needing a
    // shell at all.
    match request(std::env::args().skip(1)) {
        Request::Run => {}

        Request::Help => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }

        Request::Version => {
            println!("rsh {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }

        Request::Benchmark => {
            return match benchmark::run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("rsh: benchmark failed: {error}");
                    ExitCode::from(1)
                }
            };
        }

        Request::UnknownOption(option) => {
            eprintln!("rsh: unknown option `{option}`");
            eprintln!("      try `rsh --help`");
            return ExitCode::from(EXIT_USAGE as u8);
        }

        Request::NotAScriptRunner(name) => {
            eprintln!("rsh: commands are read from standard input, not from a file by name");
            eprintln!("      try `rsh < {name}`");
            return ExitCode::from(EXIT_USAGE as u8);
        }
    }

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

    // Two ways to read a line, chosen once. A terminal gets raw mode, editing,
    // history, and completion; a pipe gets bytes split on newlines. The rest of
    // the loop cannot tell which it is talking to.
    let mut reader = Reader::new();
    let mut editing = interactive.then(|| Interactive::new(shell.signal_fd()));

    // Run the user's configuration, if there is any. Before the first prompt,
    // and only when interactive: a script that sourced the user's rc file would
    // behave differently depending on whose machine it ran on, which is exactly
    // what a script must not do.
    if interactive {
        run_config(&mut shell);
    }

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
        let resized = shell.take_resize();
        shell.refresh_window_size(resized);

        let read = match &mut editing {
            Some(editor) => editor.read_line(PROMPT, &mut shell),
            None => reader.next_line(),
        };

        match read {
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

    if let Some(editor) = &editing {
        editor.save_history();
    }

    // Whatever the last job did to the terminal, undo it. The shell is the only
    // thing left that knows what the settings were before.
    shell.restore_terminal();

    // Statuses are a single byte to the operating system: `exit 300` becomes 44
    // no matter what any shell does, so truncate here rather than pretend
    // otherwise. The `& 0xff` also keeps negative arguments from wrapping into
    // a nonsense `u8` cast.
    ExitCode::from((status & 0xff) as u8)
}

/// The file the shell reads at startup.
const CONFIG_FILE: &str = ".rshrc";

/// Run each line of the user's configuration file.
///
/// Every line goes through the same `run_line` a typed command does — there is
/// no separate configuration language, and no separate parser to disagree with
/// the real one. A line that fails reports itself and the rest still run, which
/// is what a user wants from a file they cannot see the effects of.
fn run_config(shell: &mut Shell) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };

    let path = std::path::PathBuf::from(home).join(CONFIG_FILE);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        // No configuration file is the normal state of a new shell.
        return;
    };

    for line in contents.lines() {
        // `exit` in a configuration file is honoured: it is a strange thing to
        // write, but refusing to do what it says would be stranger.
        if let Outcome::Exit(status) = shell.run_line(line) {
            shell.restore_terminal();
            std::process::exit(status & 0xff);
        }
    }
}
