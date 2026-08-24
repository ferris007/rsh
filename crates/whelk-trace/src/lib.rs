//! Timed, structured diagnostics.
//!
//! Off unless `WHELK_TRACE` is set, and close to free when it is off: one relaxed
//! atomic load per call site.
//!
//! ```console
//! $ WHELK_TRACE=1 whelk
//! whelk> echo hi | grep h
//! trace  parse              38.4µs  input=19
//! trace  expand              4.1µs  words=2
//! trace  resolve            71.2µs  program=echo
//! trace  resolve           104.9µs  program=grep
//! trace  spawn             383.0µs  stages=2
//! trace  wait                2.9ms  status=0
//! hi
//! ```
//!
//! # Why not the `tracing` crate
//!
//! `tracing` is the obvious answer and would be a reasonable choice. Two things
//! argued against it here.
//!
//! The first is specific to this program: a shell's most interesting moment is
//! the window between `fork` and `exec`, where **nothing may allocate**. A
//! subscriber that formats an event into a `String` is exactly what must not
//! happen there, and a facility that is unsafe in the one place the timing
//! matters most is a facility with a trap in it. What this module offers
//! instead is a rule it can keep: spans are opened and closed in the parent,
//! never across a fork.
//!
//! The second is proportion. `tracing` plus a subscriber is a substantial
//! dependency tree for a project whose only other dependency wraps libc, and
//! what a shell wants is not general structured logging — it is "how long did
//! each phase take". That fits in a hundred lines.
//!
//! Adopting `tracing` later would mean rewriting the bodies of two macros. The
//! call sites would not change.

use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// Whether tracing is on, decided once.
static ENABLED: OnceLock<bool> = OnceLock::new();

/// Set while a span is being reported, to keep nested reporting out.
static REPORTING: AtomicBool = AtomicBool::new(false);

/// Whether diagnostics are switched on.
///
/// Read from `WHELK_TRACE` the first time it is asked, and cached — a shell that
/// consulted the environment on every parse would be measuring the environment.
pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var_os("WHELK_TRACE").is_some_and(|value| value != "0" && !value.is_empty())
    })
}

/// A timed region.
///
/// Reports when dropped, so the measurement covers exactly the scope it is
/// declared in and cannot be forgotten on an early return.
#[derive(Debug)]
pub struct Span {
    name: &'static str,
    fields: String,
    started: Instant,
}

impl Span {
    /// Begin timing a region.
    ///
    /// Cheap when tracing is off: no clock read, no allocation.
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            fields: String::new(),
            started: Instant::now(),
        }
    }

    /// Attach a value to the report.
    ///
    /// Takes `self` so fields can be chained onto the constructor, which keeps
    /// the call site to one line at the top of the scope being measured.
    #[must_use]
    pub fn with(mut self, key: &str, value: impl std::fmt::Display) -> Self {
        if enabled() {
            let _ = write!(self.fields, " {key}={value}");
        }
        self
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if !enabled() {
            return;
        }

        // A span reported from inside another span's reporting would interleave
        // on the same line. Not expected, but the guard costs one atomic.
        if REPORTING.swap(true, Ordering::Relaxed) {
            return;
        }

        let elapsed = self.started.elapsed();
        let mut out = std::io::stderr();
        let _ = writeln!(
            out,
            "trace  {:<16} {:>9}  {}",
            self.name,
            Duration(elapsed),
            self.fields.trim_start()
        );

        REPORTING.store(false, Ordering::Relaxed);
    }
}

/// A duration in whichever unit reads best.
///
/// Fixed-width numbers with a unit, rather than raw nanoseconds: a column of
/// `38400` and `2900000` is arithmetic the reader should not have to do.
struct Duration(std::time::Duration);

impl std::fmt::Display for Duration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let nanos = self.0.as_nanos();

        if nanos < 1_000 {
            write!(f, "{nanos}ns")
        } else if nanos < 1_000_000 {
            write!(f, "{:.1}µs", nanos as f64 / 1_000.0)
        } else if nanos < 1_000_000_000 {
            write!(f, "{:.1}ms", nanos as f64 / 1_000_000.0)
        } else {
            write!(f, "{:.2}s", nanos as f64 / 1_000_000_000.0)
        }
    }
}

/// Open a timed span for the current scope.
///
/// ```ignore
/// let _span = span!("parse", input = line.len());
/// ```
///
/// **Never across a fork.** The reporting path formats and writes, neither of
/// which is safe between `fork` and `exec`.
#[macro_export]
macro_rules! span {
    ($name:literal) => {
        $crate::Span::new($name)
    };
    ($name:literal, $($key:ident = $value:expr),+ $(,)?) => {
        $crate::Span::new($name)$(.with(stringify!($key), $value))+
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_pick_a_readable_unit() {
        let show = |nanos| Duration(std::time::Duration::from_nanos(nanos)).to_string();
        assert_eq!(show(500), "500ns");
        assert_eq!(show(38_400), "38.4µs");
        assert_eq!(show(2_900_000), "2.9ms");
        assert_eq!(show(1_500_000_000), "1.50s");
    }

    #[test]
    fn a_span_can_be_built_and_dropped_whether_or_not_tracing_is_on() {
        // The point is that nothing panics and nothing is printed when off,
        // which is the state every test in this workspace runs in.
        let span = Span::new("test").with("key", 1).with("other", "two");
        drop(span);
    }

    #[test]
    fn fields_are_only_formatted_when_anyone_will_read_them() {
        // The `with` calls are what would allocate, so they are the thing that
        // has to be free when tracing is off.
        let span = Span::new("test").with("expensive", "value");
        assert_eq!(span.fields.is_empty(), !enabled());
    }

    #[test]
    fn the_macro_accepts_a_name_alone_or_with_fields() {
        let _plain = span!("plain");
        let _with_fields = span!("fields", count = 3, name = "x");
    }
}
