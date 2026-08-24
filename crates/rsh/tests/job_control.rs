//! Job control, driven through a pseudoterminal.
//!
//! These tests need a terminal, and not as a convenience: job control *is*
//! terminal ownership. A shell reading from a pipe has none of it and correctly
//! refuses to pretend.

mod common;

use common::{Terminal, CTRL_C, CTRL_Z};

// ---- background jobs -------------------------------------------------------

#[test]
fn a_background_job_is_announced_and_listed() {
    let mut session = Terminal::open();
    session.type_in("sleep 30 &\n");
    session.expect("[1]");

    session.type_in("jobs\n");
    session.expect("Running");
    session.expect("sleep 30");
}

#[test]
fn a_finished_background_job_is_reported_at_the_next_prompt() {
    // Not the instant it happens: a notification arriving mid-keystroke would
    // write over whatever the user was typing.
    let mut session = Terminal::open();
    session.type_in("sleep 0.2 &\n");
    session.expect_eventually("Done");
}

// ---- suspend and resume ----------------------------------------------------

#[test]
fn ctrl_z_suspends_a_foreground_command() {
    let mut session = Terminal::open();
    session.type_in("sleep 30\n");
    session.type_in(CTRL_Z);
    session.expect("Stopped");

    session.type_in("jobs\n");
    session.expect("Stopped");
}

#[test]
fn bg_resumes_a_stopped_job_without_the_terminal() {
    let mut session = Terminal::open();
    session.type_in("sleep 30\n");
    session.type_in(CTRL_Z);
    session.expect("Stopped");

    session.type_in("bg\n");
    session.type_in("jobs\n");
    session.expect("Running");
}

#[test]
fn fg_brings_a_stopped_job_back_and_waits_for_it() {
    let mut session = Terminal::open();

    // Long enough that it is still there when Ctrl-Z arrives. A short sleep
    // would finish first and the test would suspend an empty prompt instead.
    session.type_in("sleep 30\n");
    session.type_in(CTRL_Z);
    session.expect("Stopped");

    // `fg` echoes the command it is resuming, then blocks — which is the half
    // that distinguishes it from `bg`.
    session.type_in("fg\n");
    session.expect("sleep 30");

    // The shell is waiting, so Ctrl-C has to go to the job to get it back.
    session.type_in(CTRL_C);
    session.type_in("echo back\n");
    session.expect("back");

    // Having been waited for, the job is gone rather than left in the table.
    session.type_in("jobs\n");
    session.type_in("echo listed\n");
    session.expect("listed");
}

// ---- process groups --------------------------------------------------------

#[test]
fn ctrl_c_kills_the_foreground_job_and_leaves_the_shell() {
    let mut session = Terminal::open();
    session.type_in("sleep 30\n");
    session.type_in(CTRL_C);

    // The shell is still there to answer.
    session.type_in("echo status=$?\n");
    session.expect("status=130");
}

#[test]
fn ctrl_c_does_not_reach_a_background_job() {
    // The whole point of giving each job its own process group. The signal goes
    // to the foreground group, and a background job is not in it.
    let mut session = Terminal::open();
    session.type_in("sleep 30 &\n");
    session.type_in("sleep 30\n");
    session.type_in(CTRL_C);

    session.type_in("jobs\n");
    session.expect("Running");
}

// ---- leaving ---------------------------------------------------------------

#[test]
fn exiting_with_a_stopped_job_warns_first() {
    // A stopped job would be left suspended with nothing able to resume it.
    // The shell states the consequence; the decision stays with the user.
    let mut session = Terminal::open();
    session.type_in("sleep 30\n");
    session.type_in(CTRL_Z);
    session.expect("Stopped");

    session.type_in("exit\n");
    session.expect("there are stopped jobs");

    // Still running, having declined to leave.
    session.type_in("echo still here\n");
    session.expect("still here");
}

#[test]
fn a_running_background_job_does_not_block_exit() {
    // It carries on perfectly well without a shell; only a *stopped* job is a
    // process leaked in a state the user cannot see.
    let mut session = Terminal::open();
    session.type_in("sleep 30 &\n");
    session.type_in("exit\n");
    assert!(
        !session.output().contains("stopped jobs"),
        "a running job should not block exit:\n{}",
        session.output()
    );
}
