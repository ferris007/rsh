//! The observation, as a test.
//!
//! `dup2(fd, fd)` leaving FD_CLOEXEC alone is the kind of detail that is easy
//! to read past in the man page and expensive to rediscover. It cost this
//! project a real bug — `sh -c '...' 3> f` silently doing nothing — so the
//! behaviour is pinned here rather than trusted to memory.

use std::process::Command;

const XP: &str = env!("CARGO_BIN_EXE_xp-file-descriptors");

fn run() -> String {
    let dir = std::env::temp_dir().join(format!("rsh-fd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("failed to create scratch dir");
    let path = dir.join("target.txt");

    let out = Command::new(XP)
        .arg(&path)
        .output()
        .expect("failed to spawn experiment");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        out.status.success(),
        "experiment exited with {:?}",
        out.status
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn dup2_onto_itself_does_not_clear_close_on_exec() {
    let output = run();
    assert!(
        output.contains("after dup2(3, 3) -> 3, FD_CLOEXEC: set"),
        "output was:\n{output}"
    );
}

#[test]
fn dup2_onto_a_different_descriptor_does_clear_it() {
    let output = run();
    assert!(
        output.contains("after dup2(3, 4) -> 4, FD_CLOEXEC: clear"),
        "output was:\n{output}"
    );
}

#[test]
fn the_flag_decides_whether_the_descriptor_survives_exec() {
    let output = run();
    assert!(
        output.contains("the descriptor was closed by exec"),
        "output was:\n{output}"
    );
    assert!(
        output.contains("the descriptor survived exec"),
        "output was:\n{output}"
    );
}
