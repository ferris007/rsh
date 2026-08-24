//! The cost of starting a process, which dominates everything else.
//!
//! These numbers are large and noisy compared to the parsing ones: they involve
//! the kernel, the loader, and another program's startup. They are here because
//! the comparison is the point — parsing a command line costs microseconds and
//! running it costs milliseconds, so a shell that optimised its parser would be
//! polishing a rounding error.

use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Duration;

fn path_lookup(c: &mut Criterion) {
    let path = std::env::var_os("PATH");

    let mut group = c.benchmark_group("resolve");

    // The difference between these two is the whole reason a shell caches:
    // an absolute path is a single stat, and a bare name is one per PATH entry
    // until it hits.
    group.bench_function("absolute", |b| {
        b.iter(|| whelk_process::resolve(black_box("/bin/sh"), path.as_deref()));
    });

    group.bench_function("search-path", |b| {
        b.iter(|| whelk_process::resolve(black_box("sh"), path.as_deref()));
    });

    // The expensive case, and the one a user hits by typing badly: a lookup
    // that fails has to exhaust PATH before it can say so.
    group.bench_function("not-found", |b| {
        b.iter(|| whelk_process::resolve(black_box("definitely-not-a-command"), path.as_deref()));
    });

    group.finish();

    // What the shell does *after* a failed lookup, to offer "did you mean".
    // It reads every directory on PATH rather than stat-ing one name in each,
    // so it is worth knowing whether that is a rounding error or the main cost.
    c.bench_function("suggest", |b| {
        b.iter(|| whelk_process::suggest(black_box("definitely-not-a-command"), path.as_deref()));
    });
}

fn spawning(c: &mut Criterion) {
    let program = PathBuf::from("/bin/true");
    // Fewer samples than criterion's default: each one forks, execs, and reaps
    // a real process, and a hundred of those is a second of wall clock.
    let mut group = c.benchmark_group("spawn");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("fork-exec-wait", |b| {
        b.iter(|| {
            let command = whelk_process::Command::new(&program, ["true"]).expect("prepared");
            let child = command.spawn().expect("spawned");
            child.wait().expect("waited")
        });
    });

    group.finish();
}

fn pipes(c: &mut Criterion) {
    // Creating the pipes is the shell's own cost in a pipeline, separate from
    // the processes on either end.
    c.bench_function("pipe/create", |b| {
        b.iter(|| whelk_process::Pipe::new().expect("created"));
    });
}

criterion_group!(benches, path_lookup, spawning, pipes);
criterion_main!(benches);
