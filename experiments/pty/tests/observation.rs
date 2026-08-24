//! The observation, as a test.
//!
//! "Why did my output stop when I piped it?" is one of the most common
//! confusions in Unix, and the answer is a rule nobody is told: C's standard
//! library changes its buffering based on whether the descriptor is a terminal.

use std::process::Command;

const XP: &str = env!("CARGO_BIN_EXE_xp-pty");

fn run() -> String {
    let out = Command::new(XP)
        .output()
        .expect("failed to spawn experiment");
    assert!(
        out.status.success(),
        "experiment exited with {:?}",
        out.status
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn output_to_a_pipe_arrives_only_at_the_end() {
    let output = run();
    let pipe = output
        .split("with output on a pseudoterminal")
        .next()
        .expect("expected a pipe section");

    assert!(
        pipe.contains("after the pause: nothing yet"),
        "a fully buffered program should have sent nothing yet:\n{output}"
    );
}

#[test]
fn output_to_a_terminal_arrives_line_by_line() {
    let output = run();
    let pty = output
        .split("with output on a pseudoterminal")
        .nth(1)
        .expect("expected a pseudoterminal section");

    assert!(
        pty.contains(r#"after the pause: "first"#),
        "a line-buffered program should have sent its first line:\n{output}"
    );
}

#[test]
fn both_arrive_in_the_end() {
    // The bytes are never lost — only delayed. Which is exactly what makes the
    // problem so confusing to diagnose from the far end of the pipe.
    assert_eq!(
        run()
            .matches(r#"in the end:      "first\nsecond\n""#)
            .count(),
        2,
        "both runs should produce the same output eventually"
    );
}
