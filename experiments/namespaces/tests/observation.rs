//! The observation, as a test.
//!
//! Skipped where the feature does not exist, or where the kernel has
//! unprivileged user namespaces switched off — which some distributions do.
//! A test that failed for that reason would be reporting the machine's policy
//! as a defect in the experiment.

use std::process::Command;

const XP: &str = env!("CARGO_BIN_EXE_xp-namespaces");

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

/// Whether this machine can actually run the demonstration.
fn available(output: &str) -> bool {
    !output.contains("Linux feature") && !output.contains("unshare failed")
}

#[test]
fn unshare_does_not_move_the_process_that_calls_it() {
    let output = run();
    if !available(&output) {
        return;
    }

    let pid_of = |line: &str| -> Option<&str> {
        output
            .lines()
            .find(|l| l.starts_with(line))?
            .rsplit(' ')
            .next()
    };

    let before = pid_of("before unshare").expect("expected a before line");
    let after = pid_of("after unshare").expect("expected an after line");

    assert_eq!(
        before, after,
        "the calling process should keep its pid:\n{output}"
    );
}

#[test]
fn a_child_born_afterwards_is_pid_one() {
    let output = run();
    if !available(&output) {
        return;
    }

    assert!(
        output.contains("the child, however, is pid 1"),
        "the child should be init for the new namespace:\n{output}"
    );
}

#[test]
fn the_parent_sees_a_different_number_for_the_same_process() {
    let output = run();
    if !available(&output) {
        return;
    }

    let outside = output
        .lines()
        .find(|line| line.starts_with("and the parent sees"))
        .expect("expected a parent line");

    assert!(
        !outside.ends_with(" 1"),
        "the same process should have two numbers:\n{output}"
    );
}
