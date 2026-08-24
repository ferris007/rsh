//! Terminal handling, driven through a pseudoterminal.
//!
//! The subject here is state that outlives the process that changed it. A job
//! that turns off echo and exits has left the terminal that way; the shell is
//! the only thing still running that knows what it was before.

mod common;

use common::Terminal;

// ---- terminal modes --------------------------------------------------------

#[test]
fn a_job_cannot_leave_the_terminal_without_echo() {
    // `stty -echo` turns off the driver's echoing and exits without undoing it.
    // If the shell did nothing, the next line the user typed would not appear —
    // the familiar "my terminal stopped showing what I type".
    //
    // The assertion is that the *typed text* comes back, which only happens if
    // echo is on. `dash` fails this exact test.
    let mut session = Terminal::open();
    session.type_in("stty -echo\n");
    session.type_in("echo restored\n");

    session.expect("echo restored");
    session.expect("restored\n");
}

#[test]
fn a_job_cannot_leave_the_terminal_in_raw_mode() {
    // Raw mode also stops the driver turning Enter into a line break, so a
    // shell that did not restore it would never see a complete line again.
    let mut session = Terminal::open();
    session.type_in("stty raw\n");
    session.type_in("echo still working\n");
    session.expect("still working");
}

#[test]
fn the_shell_keeps_working_after_several_jobs_change_modes() {
    let mut session = Terminal::open();
    session.type_in("stty -echo\n");
    session.type_in("stty raw\n");
    session.type_in("stty -echo\n");
    session.type_in("echo fine\n");
    session.expect("fine");
}

// ---- window size -----------------------------------------------------------

#[test]
fn columns_and_lines_describe_the_terminal() {
    // Children read these, so a shell that never set them would hand every
    // program a stale idea of the window.
    let mut session = Terminal::open();
    session.resize(24, 100);
    session.type_in("\n");
    session.type_in("echo size=${COLUMNS}x${LINES}\n");
    session.expect("size=100x24");
}

#[test]
fn resizing_the_window_updates_them() {
    let mut session = Terminal::open();
    session.resize(24, 100);
    session.type_in("\n");
    session.type_in("echo before=${COLUMNS}\n");
    session.expect("before=100");

    // The signal handler only sets a flag — `ioctl` is not a call a handler may
    // make — so the new size is picked up at the next prompt.
    session.resize(40, 132);
    session.type_in("\n");
    session.type_in("echo after=${COLUMNS}x${LINES}\n");
    session.expect("after=132x40");
}

#[test]
fn a_child_sees_the_size_too() {
    let mut session = Terminal::open();
    session.resize(30, 90);
    session.type_in("\n");
    session.type_in("sh -c 'echo child=$COLUMNS'\n");
    session.expect("child=90");
}
