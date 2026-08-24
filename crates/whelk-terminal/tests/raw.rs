//! Raw mode, tested against a real terminal.
//!
//! This crate's functions operate on descriptor 0, so testing them needs a
//! process whose descriptor 0 *is* a terminal. `forkpty` provides exactly that:
//! the child gets a fresh pseudoterminal as its standard input, does the work,
//! and reports back through it.
//!
//! The capability is not used by the REPL yet — line editing is Phase 8 — which
//! is precisely why it is worth testing now. An untested capability adopted a
//! phase later is a capability debugged a phase later.

use std::io::{Read, Write};

use nix::pty::{forkpty, ForkptyResult};
use nix::sys::termios::{tcgetattr, LocalFlags};
use nix::sys::wait::waitpid;

/// Run a closure in a child that owns a pseudoterminal, and return what it
/// printed.
fn with_a_terminal(body: fn()) -> String {
    // SAFETY: `forkpty` forks. The child branch runs `body` and always leaves
    // through `_exit`, never returning into the harness.
    let result = unsafe { forkpty(None, None) }.expect("failed to open a pseudoterminal");

    match result {
        ForkptyResult::Child => {
            body();
            // SAFETY: `_exit` terminates without flushing inherited buffers.
            unsafe { nix::libc::_exit(0) }
        }
        ForkptyResult::Parent { child, master } => {
            let mut transcript = String::new();
            let mut reader = std::fs::File::from(master);
            let _ = reader.read_to_string(&mut transcript);
            let _ = waitpid(child, None);
            transcript.replace('\r', "")
        }
    }
}

/// Report which of the interesting local flags are currently set.
///
/// Written straight to descriptor 1 rather than with `println!`. The test
/// harness captures `print!` by swapping a thread-local sink, so a `println!`
/// here would land in the harness's buffer instead of the pseudoterminal — and
/// the parent, reading the terminal, would see nothing at all. The symptom is a
/// test that passes under `--nocapture` and fails without it.
fn describe_flags() {
    let modes = tcgetattr(std::io::stdin()).expect("no terminal");
    let flags = modes.local_flags;
    let line = format!(
        "canonical={} echo={} signals={}\n",
        flags.contains(LocalFlags::ICANON),
        flags.contains(LocalFlags::ECHO),
        flags.contains(LocalFlags::ISIG),
    );

    let mut out = std::io::stdout();
    out.write_all(line.as_bytes())
        .expect("failed to write to the terminal");
    out.flush().expect("failed to flush");
}

// One test rather than four, because each scenario forks from a multi-threaded
// test harness. The child allocates while formatting its report, which is only
// safe because nothing else is running in it — and running four of these at
// once, each inheriting the others' descriptors, is not a bet worth taking for
// four shorter test names.
#[test]
fn raw_mode_changes_what_the_driver_does_and_puts_it_back() {
    // A terminal starts out canonical: the driver assembles lines, echoes what
    // is typed, and turns Ctrl-C into a signal.
    let untouched = with_a_terminal(describe_flags);
    assert!(
        untouched.contains("canonical=true echo=true signals=true"),
        "a fresh terminal should be canonical:\n{untouched}"
    );

    // Raw mode turns all three off. That is what an editor needs, and what a
    // shell needs to implement its own line editing in Phase 8.
    let raw = with_a_terminal(|| {
        let _guard = whelk_terminal::raw().expect("failed to enter raw mode");
        describe_flags();
    });
    assert!(
        raw.contains("canonical=false echo=false signals=false"),
        "raw mode should turn off line editing, echo, and signals:\n{raw}"
    );

    // Dropping the guard puts them back. This is the whole reason it is a
    // guard: the restore has to happen on every path out, including a panic.
    let restored = with_a_terminal(|| {
        {
            let _guard = whelk_terminal::raw().expect("failed to enter raw mode");
        }
        describe_flags();
    });
    assert!(
        restored.contains("canonical=true echo=true signals=true"),
        "dropping the guard should restore the terminal:\n{restored}"
    );

    // And a snapshot taken by hand can be put back by hand, which is what the
    // shell does around every foreground job.
    let by_hand = with_a_terminal(|| {
        let saved = whelk_terminal::snapshot().expect("no terminal");
        std::mem::forget(whelk_terminal::raw().expect("failed to enter raw mode"));
        whelk_terminal::restore(&saved).expect("failed to restore");
        describe_flags();
    });
    assert!(
        by_hand.contains("canonical=true echo=true signals=true"),
        "an explicit restore should work too:\n{by_hand}"
    );
}
