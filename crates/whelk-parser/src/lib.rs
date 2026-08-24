//! Turning shell input into structure.
//!
//! This crate never touches the operating system. It takes a `&str` and returns
//! a syntax tree, which means every case it handles can be tested without
//! spawning a process, reading a file, or having an environment.
//!
//! ```
//! let pipeline = whelk_parser::parse("echo hello | grep hello > out.txt")
//!     .unwrap()
//!     .unwrap();
//! assert_eq!(pipeline.commands().len(), 2);
//! assert_eq!(pipeline.commands()[1].redirects().len(), 1);
//! ```
//!
//! # What it does and does not do
//!
//! Quoting, escaping, and the *recognition* of parameter references happen
//! here. Their *evaluation* does not: `$HOME` becomes a
//! [`WordPart::Parameter`], never a path, because resolving it means reading
//! the environment. The executor does that, against an environment it can be
//! handed — which is what makes expansion testable without a real process.
//!
//! Syntax `whelk` does not implement yet is recognised and refused, with the
//! roadmap phase that will deliver it. It is never silently treated as
//! ordinary text. A shell that quietly handed `>` to `echo` as an argument
//! would be lying about what it does.
//!
//! Not implemented, and reported as such: `&&`, `||`, `;`, `&`, here-documents,
//! subshells, command substitution, and parameter forms beyond plain
//! `${name}`. Globbing is not implemented either, but patterns pass through
//! unchanged — which is what a POSIX shell does when a pattern matches nothing.

mod ast;
mod error;
mod lexer;
mod parser;
mod span;
mod token;
mod word;

pub use ast::{Command, Pipeline, Redirect, RedirectKind};
pub use error::ParseError;
pub use span::Span;
pub use token::{Token, TokenKind};
pub use word::{Parameter, Word, WordPart};

/// Parse a line of shell input.
///
/// Returns `Ok(None)` for input with no command in it — an empty line,
/// whitespace, or a comment. That is not an error: it is what the user typed,
/// and the REPL should prompt again without disturbing `$?`.
pub fn parse(input: &str) -> Result<Option<Pipeline>, ParseError> {
    parser::parse(lexer::tokenize(input)?)
}

/// Split a line into tokens, without building a tree.
///
/// Exposed for tests and for anything that wants to inspect the shell's view of
/// a line — the lexer is where a shell's surprises live, so being able to look
/// at it directly is worth the extra public item.
pub fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    lexer::tokenize(input)
}
