//! The tokens the lexer produces.
//!
//! Splitting lexing from parsing matters more in a shell than in most
//! languages, because a shell's tokens are not context-free: `2` is an ordinary
//! word in `echo 2` and a file descriptor number in `2>err`. Deciding that
//! while scanning characters — where "immediately followed by `>`" is a
//! question you can actually answer — keeps the grammar above it simple.

use std::fmt;

use crate::span::Span;
use crate::word::Word;

/// A token and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// What the token is.
    pub kind: TokenKind,
    /// Where it appeared in the input line.
    pub span: Span,
}

impl Token {
    /// Build a token.
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}

/// What a token is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// A word: a command name, an argument, or a redirection target.
    Word(Word),

    /// Digits immediately followed by a redirection operator, as in the `2` of
    /// `2>err`.
    ///
    /// Only unquoted digits with no space before the operator qualify, which is
    /// why `echo 2 > f` redirects stdout and prints `2`, while `echo 2> f`
    /// redirects stdout and prints nothing.
    IoNumber(i32),

    /// `|`
    Pipe,
    /// `<`
    Less,
    /// `>`
    Great,
    /// `>>`
    DGreat,
    /// `<<`
    DLess,
    /// `<&`
    LessAnd,
    /// `>&`
    GreatAnd,
    /// `&&`
    AndIf,
    /// `||`
    OrIf,
    /// `;`
    Semi,
    /// `&`
    Amp,
}

impl TokenKind {
    /// The token as the user would have typed it, for error messages.
    ///
    /// Words render as `word` rather than their text: an error that says
    /// ``unexpected `|` `` is useful, and one that quotes back a 200-character
    /// argument is not.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Word(_) => "word",
            Self::IoNumber(_) => "file descriptor number",
            Self::Pipe => "|",
            Self::Less => "<",
            Self::Great => ">",
            Self::DGreat => ">>",
            Self::DLess => "<<",
            Self::LessAnd => "<&",
            Self::GreatAnd => ">&",
            Self::AndIf => "&&",
            Self::OrIf => "||",
            Self::Semi => ";",
            Self::Amp => "&",
        }
    }

    /// Whether this token starts a redirection.
    pub fn is_redirect_operator(&self) -> bool {
        matches!(
            self,
            Self::Less | Self::Great | Self::DGreat | Self::DLess | Self::LessAnd | Self::GreatAnd
        )
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}
