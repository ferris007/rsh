//! The observation, as a test.
//!
//! An experiment whose conclusion is only written down in prose rots the first
//! time a platform changes its mind. This one fails loudly instead.

use std::process::Command;

const XP: &str = env!("CARGO_BIN_EXE_xp-fork-exec");

fn markers(mode: &str) -> usize {
    let out = Command::new(XP)
        .arg(mode)
        .output()
        .expect("failed to spawn experiment");
    assert!(
        out.status.success(),
        "experiment exited with {:?}",
        out.status
    );
    String::from_utf8_lossy(&out.stdout)
        .matches("[buffered]")
        .count()
}

#[test]
fn exit_in_the_child_duplicates_the_parents_buffered_output() {
    // The child never wrote this text. It inherited a copy of the buffer
    // holding it, and `exit` flushed that copy on the way out.
    assert_eq!(markers("exit"), 2);
}

#[test]
fn underscore_exit_in_the_child_does_not() {
    assert_eq!(markers("_exit"), 1);
}

#[test]
fn usage_is_reported_for_an_unknown_mode() {
    let out = Command::new(XP)
        .arg("nonsense")
        .output()
        .expect("failed to spawn experiment");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
}
