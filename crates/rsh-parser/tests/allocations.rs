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

/// One test, because the counter is process-global and the harness is
/// multi-threaded.
///
/// Run in parallel, these measurements include whatever a neighbouring test
/// happened to allocate — which is how parsing `ls` measures 8 alone and 64
/// beside one other test. A thread-local counter would be the alternative, and
/// touching thread-local storage from inside the global allocator is a way to
/// recurse into it.
///
/// So: every assertion about allocation lives in this function.
#[test]
fn parsing_stays_within_its_allocation_budget() {
    for (name, line, ceiling) in CASES {
        let count = allocations(|| {
            let _ = rsh_parser::parse(line);
        });

        assert!(
            count <= *ceiling,
            "parsing the {name} case allocated {count} times, over the ceiling of {ceiling}.\n\
             line: {line}\n\
             If the change was deliberate, raise the number and say why."
        );
    }

    // Ten identical lines should cost about ten times one line. A parser that
    // had become quadratic would pass every ceiling above.
    let one = allocations(|| {
        let _ = rsh_parser::parse("echo hello world");
    });

    let ten = allocations(|| {
        for _ in 0..10 {
            let _ = rsh_parser::parse("echo hello world");
        }
    });

    assert!(
        ten <= one * 12,
        "ten lines allocated {ten} times against {one} for one — that is not flat"
    );
}

/// Print the measured counts, for setting the ceilings above from data.
///
/// `cargo test -p rsh-parser --test allocations -- --ignored --nocapture`
#[test]
#[ignore = "reports numbers rather than asserting"]
fn report_measured_counts() {
    for (name, line, ceiling) in CASES {
        let count = allocations(|| {
            let _ = rsh_parser::parse(line);
        });
        println!("{name:<12} {count:>4}  (ceiling {ceiling})  {line}");
    }
}
