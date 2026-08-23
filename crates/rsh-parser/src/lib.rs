//! Turning shell input into structure.
//!
//! This crate never touches the operating system. It takes a `&str` and
//! returns an AST, which means every case it handles can be tested without
//! spawning a process or touching the filesystem.
//!
//! # Scope
//!
//! Phase 1 recognises exactly one construct: a simple command, with quoting
//! and escaping so that `echo "hello world"` means what it looks like. Words
//! are *not* expanded — `$HOME` is currently a literal `$HOME`, and expansion
//! arrives in Phase 2 alongside the real lexer.
//!
//! Operators (`|`, `>`, `<`, `&`, `;`) are deliberately recognised and
//! **rejected** rather than passed through as ordinary words. A shell that
//! silently handed `>` to `echo` as an argument would be quietly lying about
//! what it does; refusing with a message that names the phase the feature
//! lands in is honest and, for a project like this, more useful.

mod ast;
mod error;
mod lexer;

pub use ast::Command;
pub use error::ParseError;

/// Parse a line of shell input.
///
/// Returns `Ok(None)` for input that contains no command at all — an empty
/// line, or whitespace. That is not an error: it is what the user typed, and
/// the REPL should simply prompt again without disturbing `$?`.
pub fn parse(input: &str) -> Result<Option<Command>, ParseError> {
    let words = lexer::split(input)?;
    Ok(Command::from_words(words))
}
