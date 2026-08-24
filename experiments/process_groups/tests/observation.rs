//! The observation, as a test.
//!
//! `SIGTTIN` explains a behaviour every shell user has seen and few can name:
//! `cat &` stops the instant it tries to read. Pinning it here keeps the
//! explanation honest as the shell's terminal handling grows in Phase 7.

use std::process::Command;

const XP: &str = env!("CARGO_BIN_EXE_xp-process-groups");

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
fn the_foreground_group_may_read_the_terminal() {
    let output = run();
    assert!(
        output.contains("foreground: running"),
        "output was:\n{output}"
    );
}

#[test]
fn a_background_group_is_stopped_for_trying() {
    let output = run();
    assert!(
        output.contains("background: stopped by SIGTTIN"),
        "output was:\n{output}"
    );
}
