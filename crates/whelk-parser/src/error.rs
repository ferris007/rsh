//! Parse failures.
//!
//! Every variant carries a [`Span`], so the shell can underline the exact
//! characters at fault. That is the entire reason spans exist in this crate.

use std::fmt;

use crate::span::Span;

/// Why a line could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A quoted string ran to the end of the line without closing.
    ///
    /// A real shell would keep reading with a continuation prompt. `whelk` has no
    /// multi-line input yet, so it reports instead of pretending.
    UnterminatedQuote {
        /// The quote left open: `'` or `"`.
        quote: char,
        /// The opening quote.
        span: Span,
    },

    /// The line ended with a `\`, which escapes the newline.
    TrailingBackslash {
        /// The backslash.
        span: Span,
    },

    /// A `${` with no closing `}`.
    UnterminatedBrace {
        /// The opening `${`.
        span: Span,
    },

    /// A parameter reference the shell understands the shape of but cannot
    /// evaluate — `$1`, `${X:-default}`.
    ///
    /// Distinguished from a plain syntax error because the user wrote
    /// something real; it is `whelk` that is incomplete.
    UnsupportedParameter {
        /// What was written.
        text: String,
        /// Why it cannot be used.
        reason: &'static str,
        /// The parameter reference.
        span: Span,
    },

    /// Syntax `whelk` recognises but has not implemented.
    Unsupported {
        /// The construct as written.
        token: String,
        /// The roadmap phase that will implement it, if it is on the roadmap.
        phase: Option<u8>,
        /// Where it appeared.
        span: Span,
    },

    /// A token that cannot appear where it did.
    UnexpectedToken {
        /// A short description of the token.
        token: String,
        /// Where it appeared.
        span: Span,
    },

    /// A pipeline segment with nothing in it: `| grep x`, or `echo hi |`.
    MissingCommand {
        /// The pipe that has no command on one side.
        span: Span,
    },

    /// A redirection with no file or descriptor after it.
    MissingRedirectTarget {
        /// The operator that was left dangling.
        operator: String,
        /// Where it appeared.
        span: Span,
    },
}

impl ParseError {
    /// The characters at fault.
    pub fn span(&self) -> Span {
        match self {
            Self::UnterminatedQuote { span, .. }
            | Self::TrailingBackslash { span }
            | Self::UnterminatedBrace { span }
            | Self::UnsupportedParameter { span, .. }
            | Self::Unsupported { span, .. }
            | Self::UnexpectedToken { span, .. }
            | Self::MissingCommand { span }
            | Self::MissingRedirectTarget { span, .. } => *span,
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedQuote { quote, .. } => write!(f, "unterminated {quote} quote"),
            Self::TrailingBackslash { .. } => write!(f, "line ends with an unescaped `\\`"),
            Self::UnterminatedBrace { .. } => write!(f, "unterminated `${{`"),
            Self::UnsupportedParameter { text, reason, .. } => {
                write!(f, "`{text}`: {reason}")
            }
            Self::Unsupported {
                token,
                phase: Some(phase),
                ..
            } => {
                write!(f, "`{token}` is not supported yet (roadmap phase {phase})")
            }
            Self::Unsupported {
                token, phase: None, ..
            } => {
                write!(f, "`{token}` is not supported")
            }
            Self::UnexpectedToken { token, .. } => write!(f, "unexpected {token}"),
            Self::MissingCommand { .. } => write!(f, "missing command around `|`"),
            Self::MissingRedirectTarget { operator, .. } => {
                write!(f, "`{operator}` needs a target")
            }
        }
    }
}

impl std::error::Error for ParseError {}
