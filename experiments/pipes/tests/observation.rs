//! The observation, as a test.
//!
//! `SIG_IGN` surviving `exec` is the detail that makes this bite. It is one
//! sentence in `execve(2)` and it silently changes the behaviour of every
//! program a Rust-written shell runs.

use std::process::Command;

const XP: &str = env!("CARGO_BIN_EXE_xp-pipes");

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
fn a_rust_program_ignores_sigpipe_before_main_runs() {
    assert!(
        run().contains("this process ignores SIGPIPE: true"),
        "output was:\n{}",
        run()
    );
}

#[test]
fn the_ignored_disposition_survives_exec() {
    let output = run();
    assert!(
        output.contains("the signal was ignored"),
        "output was:\n{output}"
    );
}

#[test]
fn resetting_it_in_the_child_restores_the_default_action() {
    let output = run();
    assert!(
        output.contains("killed by SIGPIPE"),
        "output was:\n{output}"
    );
}
