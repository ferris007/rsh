//! Byte ranges into the source line.
//!
//! Every token, word, and error carries one. The reason is narrow and
//! practical: it is what lets the shell underline the exact characters it is
//! complaining about. Without spans, the best any parser can manage is
//! "syntax error near unexpected token" — which is what a shell says when it
//! has thrown away the information needed to explain itself.

use std::fmt;

/// A half-open byte range `[start, end)` in the input line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Byte offset of the first character.
    pub start: usize,
    /// Byte offset one past the last character.
    pub end: usize,
}

impl Span {
    /// A span covering `[start, end)`.
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end, "span start must not exceed end");
        Self { start, end }
    }

    /// A zero-width span, used to point at a place where something is missing.
    pub fn empty(at: usize) -> Self {
        Self { start: at, end: at }
    }

    /// The smallest span covering both.
    ///
    /// Used to give a parsed construct the extent of everything it was built
    /// from: a pipeline spans from its first word to its last.
    pub fn to(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Width in bytes.
    pub fn len(self) -> usize {
        self.end - self.start
    }

    /// Whether the span covers no characters.
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// The text this span refers to, if it is in range.
    pub fn slice(self, source: &str) -> Option<&str> {
        source.get(self.start..self.end)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_measure_and_slice() {
        let span = Span::new(5, 10);
        assert_eq!(span.len(), 5);
        assert!(!span.is_empty());
        assert_eq!(span.slice("echo hello there"), Some("hello"));
    }

    #[test]
    fn empty_spans_point_between_characters() {
        let span = Span::empty(4);
        assert!(span.is_empty());
        assert_eq!(span.len(), 0);
    }

    #[test]
    fn joining_covers_both_ends() {
        assert_eq!(Span::new(2, 4).to(Span::new(9, 11)), Span::new(2, 11));
        // Order does not matter.
        assert_eq!(Span::new(9, 11).to(Span::new(2, 4)), Span::new(2, 11));
    }
}
