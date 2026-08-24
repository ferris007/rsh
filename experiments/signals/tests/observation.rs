//! The observation, as a test.
//!
//! Process groups are the mechanism behind every visible fact about Ctrl-C in a
//! shell: why it reaches a whole pipeline, why it does not reach a background
//! job, and why the shell survives it. Pinning the behaviour here means the
//! Phase 6 job-control work has something to build against.

use std::process::Command;

const XP: &str = env!("CARGO_BIN_EXE_xp-signals");

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
fn a_group_signal_reaches_every_member() {
    let output = run();
    assert!(
        output.contains("same group: killed by SIGINT"),
        "output was:\n{output}"
    );
}

#[test]
fn a_child_in_its_own_group_is_untouched() {
    let output = run();
    assert!(
        output.contains("own group:  still running"),
        "output was:\n{output}"
    );
}
