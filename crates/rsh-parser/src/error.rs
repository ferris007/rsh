//! Parse failures.
//!
//! Every variant carries enough information to say something specific. "syntax
//! error near unexpected token" is a message a shell gives when it has thrown
//! away the context it needed to explain itself; the point of keeping the
//! offending text and the byte offset here is to never have to write that.

use std::fmt;

/// Why a line could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A quoted string ran to the end of the line without closing.
    ///
    /// A real shell would continue reading on the next line with a `>`
    /// continuation prompt. `rsh` does not have multi-line input yet, so it
    /// reports rather than pretending the quote closed.
    UnterminatedQuote {
        /// The quote character that was left open: `'` or `"`.
        quote: char,
        /// Byte offset where the quote was opened.
        at: usize,
    },

    /// The line ended with a `\`, which escapes the newline.
    ///
    /// Same story as the unterminated quote: this is a line-continuation
    /// request, and `rsh` cannot honour it until it can read a second line.
    TrailingBackslash {
        /// Byte offset of the backslash.
        at: usize,
    },

    /// A shell operator that `rsh` recognises but does not implement yet.
    Unsupported {
        /// The operator as written.
        token: String,
        /// The roadmap phase that will implement it.
        phase: u8,
        /// Byte offset where it appeared.
        at: usize,
    },
}

impl ParseError {
    /// Byte offset in the input line where the problem was found.
    pub fn at(&self) -> usize {
        match self {
            Self::UnterminatedQuote { at, .. }
            | Self::TrailingBackslash { at }
            | Self::Unsupported { at, .. } => *at,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedQuote { quote, .. } => {
                write!(f, "unterminated {quote} quote")
            }
            Self::TrailingBackslash { .. } => {
                write!(f, "line ends with an unescaped `\\`")
            }
            Self::Unsupported { token, phase, .. } => {
                write!(f, "`{token}` is not supported yet (roadmap phase {phase})")
            }
        }
    }
}

impl std::error::Error for ParseError {}
