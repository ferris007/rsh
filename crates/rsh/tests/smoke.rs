//! Integration-test harness.
//!
//! Tests drive the shell the way a user does: spawn the real binary, feed it
//! bytes, inspect what comes back. No test-only hooks into the shell's guts —
//! if a behaviour can't be observed from outside the process, it isn't a
//! behaviour this project claims to have.

use std::process::Command;

/// Path to the binary built for this test run, provided by Cargo.
const RSH: &str = env!("CARGO_BIN_EXE_rsh");

#[test]
fn binary_runs_and_exits_zero() {
    let out = Command::new(RSH).output().expect("failed to spawn rsh");
    assert!(out.status.success(), "rsh exited with {:?}", out.status);
    assert!(String::from_utf8_lossy(&out.stdout).contains("rsh"));
}
