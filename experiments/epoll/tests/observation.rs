//! The observation, as a test.
//!
//! A race that shows up once in thousands of runs is not something to leave to
//! a comment. This one is forced to happen every time, so the test either sees
//! it or the behaviour has changed.

use std::process::Command;

const XP: &str = env!("CARGO_BIN_EXE_xp-epoll");

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
fn a_flag_alone_lets_the_loop_sleep_through_the_signal() {
    let output = run();
    let flag_only = output
        .split("with a self-pipe")
        .next()
        .expect("expected a flag-only section");

    assert!(
        flag_only.contains("slept through a signal"),
        "the race did not reproduce:\n{output}"
    );
}

#[test]
fn a_self_pipe_wakes_it_immediately() {
    let output = run();
    let with_pipe = output
        .split("with a self-pipe")
        .nth(1)
        .expect("expected a self-pipe section");

    assert!(
        with_pipe.contains("the signal was not missed"),
        "the self-pipe did not wake the loop:\n{output}"
    );
    assert!(
        with_pipe.contains("returned after 0ms") || with_pipe.contains("returned after 1ms"),
        "the wake should be immediate:\n{output}"
    );
}
