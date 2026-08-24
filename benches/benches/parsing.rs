//! Parsing and expansion: the part of a command's cost the shell controls.
//!
//! Everything measured here is pure computation — no syscalls, no filesystem,
//! no processes. That makes it the only part of the shell whose timing is
//! stable enough for a benchmark to be worth watching over time.

use criterion::{criterion_group, criterion_main, Criterion};
use rsh_executor::{expand_all, MapEnv};
use std::hint::black_box;

/// Lines chosen to span what a shell actually sees: a bare command, a command
/// with arguments, quoting, expansion, and a pipeline with redirection.
const LINES: &[(&str, &str)] = &[
    ("bare", "ls"),
    ("arguments", "grep -rn pattern src/"),
    ("quoting", r#"echo "a $HOME b" 'literal $HOME' c\ d"#),
    ("expansion", "echo $HOME/$USER/${PATH}"),
    ("pipeline", "cat file.txt | grep rust | sort -u | head -20"),
    ("redirection", "cmd < in.txt > out.txt 2>&1 3>&2"),
];

fn parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    for (name, line) in LINES {
        group.bench_function(*name, |b| {
            b.iter(|| rsh_parser::parse(black_box(line)).expect("should parse"));
        });
    }

    group.finish();
}

fn expansion(c: &mut Criterion) {
    let env = MapEnv::new()
        .with("HOME", "/home/ferris")
        .with("USER", "ferris")
        .with("PATH", "/usr/local/bin:/usr/bin:/bin")
        .with("LIST", "one two three four five");

    let mut group = c.benchmark_group("expand");

    for (name, line) in [
        ("literal", "echo one two three"),
        ("variables", "echo $HOME/$USER"),
        ("quoted", r#"echo "$LIST""#),
        ("split", "echo $LIST"),
    ] {
        let pipeline = rsh_parser::parse(line).unwrap().unwrap();
        let words = pipeline.commands()[0].words().to_vec();

        group.bench_function(name, |b| {
            b.iter(|| expand_all(black_box(&words), &env));
        });
    }

    group.finish();
}

fn throughput(c: &mut Criterion) {
    // A whole script, to give a figure that can be compared against a file
    // size rather than a single line.
    let script: String = LINES
        .iter()
        .cycle()
        .take(100)
        .map(|(_, line)| format!("{line}\n"))
        .collect();

    c.bench_function("parse/script-100-lines", |b| {
        b.iter(|| {
            for line in black_box(&script).lines() {
                let _ = rsh_parser::parse(line);
            }
        });
    });
}

criterion_group!(benches, parsing, expansion, throughput);
criterion_main!(benches);
