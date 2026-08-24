//! What the shell does with its own command line.
//!
//! A shell is mostly a program that runs other programs, so its own arguments
//! are easy to leave half-finished — and they are the first thing a person
//! touches after installing it. These tests spawn the real binary and read what
//! it says, the same as the rest of the end-to-end suite.

use std::process::{Command, Output};

/// The binary built for this test run, provided by Cargo.
const RSH: &str = env!("CARGO_BIN_EXE_rsh");

/// Run the shell with arguments and no input at all.
fn run(arguments: &[&str]) -> Output {
    Command::new(RSH)
        .args(arguments)
        .output()
        .expect("failed to run the shell")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn help_describes_how_to_start_a_shell() {
    let output = run(&["--help"]);

    assert!(output.status.success(), "--help should succeed");
    let text = stdout(&output);
    assert!(text.contains("usage:"), "no usage section in:\n{text}");
    assert!(text.contains("rsh < script"), "no stdin form in:\n{text}");
    assert!(text.contains("--version"), "no --version in:\n{text}");
}

#[test]
fn help_says_what_the_shell_cannot_do() {
    // The absences belong in the help, not only in the README. Someone who
    // types `ls *.rs` and gets a literal `*.rs` should be able to find out why
    // without leaving the terminal.
    let text = stdout(&run(&["--help"]));

    for missing in ["&&", "globbing", "command substitution"] {
        assert!(text.contains(missing), "help does not mention {missing}");
    }
}

#[test]
fn help_has_a_short_form() {
    assert_eq!(stdout(&run(&["-h"])), stdout(&run(&["--help"])));
}

#[test]
fn version_reports_the_version_cargo_built() {
    let output = run(&["--version"]);

    assert!(output.status.success(), "--version should succeed");
    assert_eq!(
        stdout(&output).trim(),
        format!("rsh {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn version_has_a_short_form() {
    assert_eq!(stdout(&run(&["-V"])), stdout(&run(&["--version"])));
}

#[test]
fn an_unknown_option_is_reported_rather_than_ignored() {
    // The failure this guards against is silence: a shell that accepts
    // `--colour` by starting a normal session has said it understood.
    let output = run(&["--colour"]);

    assert_eq!(output.status.code(), Some(2), "usage errors report 2");
    let complaint = stderr(&output);
    assert!(
        complaint.contains("unknown option") && complaint.contains("--colour"),
        "unhelpful complaint:\n{complaint}"
    );
    assert!(complaint.contains("--help"), "no pointer to help");
}

#[test]
fn a_file_named_on_the_command_line_says_where_commands_come_from() {
    let output = run(&["script.sh"]);

    assert_eq!(output.status.code(), Some(2), "usage errors report 2");
    let complaint = stderr(&output);
    assert!(
        complaint.contains("standard input"),
        "does not explain where commands come from:\n{complaint}"
    );
    assert!(
        complaint.contains("rsh < script.sh"),
        "does not show the form that works:\n{complaint}"
    );
}

#[test]
fn no_arguments_reads_standard_input_as_before() {
    // With stdin closed, an argument-free shell reaches end of input and stops.
    // The point is that it starts at all: option handling must not swallow the
    // ordinary case.
    let output = run(&[]);

    assert!(
        output.status.success(),
        "a shell with no arguments and no input should exit cleanly, got {:?}",
        output.status
    );
}
