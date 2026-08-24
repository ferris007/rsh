//! Performance regression tests that are not about time.
//!
//! # Why not assert on timing
//!
//! A CI runner's wall clock varies by several times between runs. A threshold
//! loose enough to pass reliably there is too loose to catch anything a person
//! would care about, and a tight one fails on a busy afternoon and gets
//! disabled. Timing belongs in `cargo bench`, where a human reads the numbers.
//!
//! Allocation counts are the opposite: deterministic, machine-independent, and
//! a good proxy for the thing that actually goes wrong in a parser — a rewrite
//! that quietly starts copying a `String` per token.
//!
//! These numbers are ceilings with room in them, not targets. A change that
//! moves one is not a failure; it is a prompt to look, decide, and edit the
//! number with a reason in the commit message.
//!
//! # Why there is no test harness
//!
//! The counter below is a global in the process, so it counts every allocation
//! made anywhere while it is running — including ones made on another thread.
//! libtest is multi-threaded: it runs each test on its own thread and formats
//! results on the main one, and that formatting allocates. Measured beside one
//! other test, parsing `ls` reads anywhere between 15 and 62 rather than its
//! true 8, which is how this file first failed on CI while passing locally.
//!
//! Filtering by thread is the obvious repair and the wrong one: finding out
//! which thread you are on means touching thread-local storage from inside the
//! global allocator, and on some platforms the first such touch allocates —
//! straight back into the allocator that asked.
//!
//! So this target sets `harness = false` in `Cargo.toml` and is a plain `main`.
//! One thread, nothing else running in it, the same number every time.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Allocations since the counter was last reset.
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

/// A `System` allocator that counts.
///
/// Counting rather than measuring bytes: the number of allocations is what a
/// parser controls, and the byte count is dominated by whatever the input
/// happens to be.
struct Counting;

// SAFETY: every method forwards to `System`, which is a correct allocator. The
// counter is a relaxed atomic add, which cannot affect the allocation itself.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwarding the caller's contract unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How many allocations a closure performs.
fn allocations(body: impl FnOnce()) -> usize {
    ALLOCATIONS.store(0, Ordering::Relaxed);
    body();
    ALLOCATIONS.load(Ordering::Relaxed)
}

/// The measured counts, and the ceilings derived from them.
///
/// Ceilings are the measurement with roughly half again as headroom. They are
/// not targets: a change that moves one is a prompt to look, decide, and edit
/// the number with a reason in the commit message.
const CASES: &[(&str, &str, usize)] = &[
    ("bare", "ls", 14),
    ("arguments", "grep -rn pattern src/", 34),
    (
        "pipeline",
        "cat file.txt | grep rust | sort -u | head -20",
        66,
    ),
    ("quoting", r#"echo "a $HOME b" 'literal' c\ d"#, 40),
    ("expansion", "echo $HOME/$USER/${PATH}", 36),
];

fn main() {
    // The counts are printed whether or not they pass: the numbers are the
    // point, and a run that only says "ok" gives nobody the figure to write
    // into `CASES` after a deliberate change.
    //
    // Printing first, before anything is measured, also gets the one-time cost
    // of setting up stdout out of the way, where it cannot land in a count.
    println!("case          allocations  ceiling");

    let mut over = Vec::new();

    for (name, line, ceiling) in CASES {
        let count = allocations(|| {
            let _ = whelk_parser::parse(line);
        });

        let verdict = if count <= *ceiling { "" } else { "  OVER" };
        println!("{name:<12} {count:>11}  {ceiling:>7}{verdict}");

        if count > *ceiling {
            over.push(format!(
                "parsing the {name} case allocated {count} times, over the ceiling of {ceiling}.\n\
                 line: {line}"
            ));
        }
    }

    // Ten identical lines should cost about ten times one line. A parser that
    // had become quadratic would pass every ceiling above.
    let one = allocations(|| {
        let _ = whelk_parser::parse("echo hello world");
    });

    let ten = allocations(|| {
        for _ in 0..10 {
            let _ = whelk_parser::parse("echo hello world");
        }
    });

    println!("\nscaling      {one} for one line, {ten} for ten");

    if ten > one * 12 {
        over.push(format!(
            "ten lines allocated {ten} times against {one} for one — that is not flat"
        ));
    }

    if !over.is_empty() {
        eprintln!();
        for complaint in &over {
            eprintln!("{complaint}");
        }
        eprintln!("\nIf the change was deliberate, raise the number and say why.");
        std::process::exit(1);
    }
}
