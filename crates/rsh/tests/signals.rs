//! Signal behaviour, end to end.
//!
//! Kept apart from `repl.rs` because these tests interact with a running shell
//! rather than feeding it a script and reading the result: some of them have to
//! signal the process while it is blocked.
//!
//! Where a deterministic formulation exists it is used. A child that stops
//! *itself* proves the same thing as Ctrl-Z with none of the timing, so the
//! tests that sleep are only the ones that genuinely cannot avoid it.

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

const RSH: &str = env!("CARGO_BIN_EXE_rsh");

/// How long to give the shell to reach its blocking read.
///
/// Generous on purpose: a test that fails when a CI runner is busy teaches
/// nothing about the shell.
const SETTLE: Duration = Duration::from_millis(400);

/// Feed a script to the shell and collect everything it produced.
fn run(script: &str) -> (String, String, i32) {
    let mut child = spawn();
    child
        .stdin
        .take()
        .expect("stdin was not piped")
        .write_all(script.as_bytes())
        .expect("failed to write script");

    let out = child.wait_with_output().expect("failed to collect output");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().expect("rsh was killed by a signal"),
    )
}

/// Start a shell with its input still open, for tests that signal it.
fn spawn() -> Child {
    Command::new(RSH)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn rsh")
}

fn signal(child: &Child, signal: Signal) {
    kill(Pid::from_raw(child.id() as i32), signal).expect("failed to signal rsh");
}

// ---- what children inherit -------------------------------------------------

#[test]
fn exec_resets_the_shells_handlers_in_children() {
    // The shell handles SIGINT so Ctrl-C cannot kill it. If it had *ignored*
    // the signal instead, SIG_IGN would survive exec and every child would be
    // uninterruptible — the same trap SIGPIPE sets. A handler is reset by exec,
    // so children get the default action and die normally.
    let (_, _, status) = run("sh -c 'kill -INT $$'\n");
    assert_eq!(status, 130, "130 = 128 + SIGINT");
}

#[test]
fn the_same_holds_for_sigquit() {
    let (_, _, status) = run("sh -c 'kill -QUIT $$'\n");
    assert_eq!(status, 131, "131 = 128 + SIGQUIT");
}

// ---- stopped children ------------------------------------------------------

#[test]
fn a_stopped_child_is_continued_rather_than_stranded() {
    // Without WUNTRACED, waitpid simply does not return for a stopped child:
    // the shell blocks forever on a process that will never finish, with no
    // prompt and no way back. This test hangs if that regresses.
    //
    // The child stops itself, so there is no timing here at all.
    //
    // SIGSTOP rather than SIGTSTP, and the difference is not cosmetic: POSIX
    // says SIGTSTP, SIGTTIN, and SIGTTOU are *discarded* when sent to a member
    // of an orphaned process group. Under a CI runner the shell's group is
    // orphaned, so a SIGTSTP test quietly stops testing anything — the child
    // never suspends and the assertions on its output still pass. SIGSTOP
    // cannot be caught, blocked, or discarded.
    let (stdout, stderr, status) = run("sh -c 'kill -STOP $$; echo continued'\n");
    assert_eq!(stdout, "continued\n");
    assert_eq!(status, 0);
    assert!(
        stderr.contains("stopped"),
        "the shell said nothing: {stderr:?}"
    );
}

#[test]
fn a_stopped_stage_does_not_wedge_a_pipeline() {
    // SIGSTOP for the same reason as above: it cannot be discarded.
    let (stdout, _, _) = run("sh -c 'kill -STOP $$; echo through' | tr a-z A-Z\n");
    assert_eq!(stdout, "THROUGH\n");
}

// ---- the shell's own signals -----------------------------------------------

#[test]
fn ctrl_c_while_waiting_for_input_does_not_kill_the_shell() {
    let mut child = spawn();
    std::thread::sleep(SETTLE);

    // The shell is blocked reading. The handler runs, the read fails with
    // EINTR, and the line is abandoned — the shell must survive and keep going.
    signal(&child, Signal::SIGINT);
    std::thread::sleep(SETTLE);

    let mut stdin = child.stdin.take().expect("stdin was not piped");
    stdin.write_all(b"echo $?\n").expect("shell died on SIGINT");
    drop(stdin);

    let out = child.wait_with_output().expect("failed to collect output");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "130\n",
        "an interrupted read should report 128 + SIGINT"
    );
}

/// Signal a shell that is idle at its input, and report how it exited.
///
/// Standard input is held open across the wait. `Child::wait` drops its own
/// copy, and the resulting end-of-input would otherwise race the signal — the
/// shell would exit for the wrong reason and the test would pass or fail
/// depending on scheduling.
fn exit_status_after(sig: Signal) -> std::process::ExitStatus {
    let mut child = spawn();
    let stdin = child.stdin.take().expect("stdin was not piped");

    std::thread::sleep(SETTLE);
    signal(&child, sig);

    let status = child.wait().expect("failed to wait for rsh");
    drop(stdin);
    status
}

#[test]
fn sigterm_shuts_the_shell_down() {
    let status = exit_status_after(Signal::SIGTERM);

    // `code()` is `None` for a process killed by a signal, so this asserts two
    // things at once: the shell caught SIGTERM and exited deliberately rather
    // than simply dying, and it reported the conventional status for it.
    assert_eq!(status.code(), Some(143), "143 = 128 + SIGTERM");
}

#[test]
fn sighup_shuts_the_shell_down_too() {
    assert_eq!(
        exit_status_after(Signal::SIGHUP).code(),
        Some(129),
        "129 = 128 + SIGHUP"
    );
}
