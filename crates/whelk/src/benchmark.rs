//! `whelk --benchmark`: what the shell costs, measured on the machine it is on.
//!
//! Not a substitute for `cargo bench`, which is where regressions are caught.
//! This answers a different question — "is this shell fast enough on *this*
//! machine" — and answers it in a form a reader can compare against another
//! shell in a few seconds.
//!
//! # What is deliberately measured end to end
//!
//! Every figure below includes the kernel and, where relevant, another
//! program's startup. That is the honest unit: a user waiting for `echo hi` is
//! waiting for all of it, and a shell that reported only its own share would be
//! reporting the smallest part.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How many times to run each measurement.
///
/// Enough for a median to mean something, few enough that the whole thing
/// finishes while a person is still looking at it.
const RUNS: usize = 30;

/// Run the measurements and print the table.
pub fn run() -> std::io::Result<()> {
    let shell = std::env::current_exe()?;

    let startup = median(RUNS, || shell_runs(&shell, "exit\n"));
    let echo = median(RUNS, || shell_runs(&shell, "echo hi\n"));
    let pipeline = median(RUNS, || shell_runs(&shell, "echo hi | tr a-z A-Z | cat\n"));

    let mut out = std::io::stdout();
    writeln!(out, "whelk benchmark")?;
    writeln!(out, "────────────────────────")?;
    row(&mut out, "startup", startup)?;
    row(&mut out, "echo", echo)?;
    row(&mut out, "pipeline", pipeline)?;
    writeln!(out, "{:<12}  {:>7.1} MB", "memory", peak_memory_mb())?;

    Ok(())
}

fn row(out: &mut impl Write, name: &str, duration: Duration) -> std::io::Result<()> {
    writeln!(
        out,
        "{name:<12}  {:>7.2} ms",
        duration.as_secs_f64() * 1_000.0
    )
}

/// Time one full run of the shell over a script.
///
/// A fresh process each time, which is what makes "startup" mean anything: the
/// cost being measured is a shell someone typed a command into, not a loop
/// inside an already-warm one.
fn shell_runs(shell: &PathBuf, script: &str) -> Duration {
    use std::process::{Command, Stdio};

    let started = Instant::now();

    let mut child = Command::new(shell)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start the shell");

    {
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let _ = stdin.write_all(script.as_bytes());
    }

    let _ = child.wait();
    started.elapsed()
}

/// The middle measurement, which a slow neighbour cannot drag.
///
/// A mean would be moved by one scheduling hiccup, and on a shared machine
/// there is always one. The median says what a typical run costs, which is the
/// question being asked.
fn median(runs: usize, mut measure: impl FnMut() -> Duration) -> Duration {
    let mut samples: Vec<Duration> = (0..runs).map(|_| measure()).collect();
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Peak resident memory, in megabytes.
///
/// `getrusage` reports the high-water mark rather than the current figure,
/// which is the more useful number: a shell's cost is what it needed at its
/// worst, not what it happens to hold while printing a table.
fn peak_memory_mb() -> f64 {
    // SAFETY: `rusage` is a plain C struct of integers, for which an all-zero
    // bit pattern is a valid value. `getrusage` overwrites it below.
    let mut usage: nix::libc::rusage = unsafe { std::mem::zeroed() };

    // SAFETY: `getrusage` fills the struct through the pointer, and that is
    // what is passed. A failure leaves it zeroed, which reports 0.0.
    unsafe { nix::libc::getrusage(nix::libc::RUSAGE_SELF, &raw mut usage) };

    // Linux reports kilobytes; macOS and the BSDs report bytes. Getting this
    // wrong by a factor of 1024 is the classic way to publish a wrong number.
    let scale = if cfg!(target_os = "linux") {
        1_024.0
    } else {
        1.0
    };
    usage.ru_maxrss as f64 * scale / (1_024.0 * 1_024.0)
}
