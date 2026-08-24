//! Line editing, which happens once per keystroke.
//!
//! The budget here is different from everything else in the shell: a keystroke
//! that takes longer than a few milliseconds is one a person can feel. These
//! numbers are microseconds, and the interesting question is whether they stay
//! that way as the line and the history grow.

use criterion::{criterion_group, criterion_main, Criterion};
use rsh_line::{Editor, History, Key, NoCompletion};
use std::hint::black_box;

fn history_of(count: usize) -> History {
    let mut history = History::new(10_000);
    for n in 0..count {
        history.add(&format!("command number {n} with some arguments"));
    }
    history
}

fn typing(c: &mut Criterion) {
    let mut group = c.benchmark_group("edit");

    group.bench_function("insert-character", |b| {
        let mut editor = Editor::new(History::new(100));
        b.iter(|| editor.handle(black_box(Key::Char('x')), &NoCompletion));
    });

    group.bench_function("type-a-line", |b| {
        b.iter(|| {
            let mut editor = Editor::new(History::new(100));
            for c in black_box("git commit -m 'a reasonable message'").chars() {
                editor.handle(Key::Char(c), &NoCompletion);
            }
            editor.handle(Key::Enter, &NoCompletion)
        });
    });

    group.finish();
}

fn recall(c: &mut Criterion) {
    let mut group = c.benchmark_group("history");

    // Prefix search walks backwards until it matches, so the cost depends on
    // how much history there is and how far back the match lives. The second
    // case is the bad one: a prefix nothing matches scans the whole thing.
    for size in [100_usize, 10_000] {
        group.bench_function(format!("recent-match/{size}"), |b| {
            let history = history_of(size);
            b.iter(|| history.search_back(black_box("command number 9"), size));
        });

        group.bench_function(format!("no-match/{size}"), |b| {
            let history = history_of(size);
            b.iter(|| history.search_back(black_box("zzz"), size));
        });
    }

    group.finish();
}

criterion_group!(benches, typing, recall);
criterion_main!(benches);
