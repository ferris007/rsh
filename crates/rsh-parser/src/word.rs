//! Words, and the pieces they are made of.
//!
//! # Why a word is not a `String`
//!
//! By the time the lexer has finished, `hello` and `"$HOME/bin"` have both been
//! through quote removal — but only one of them is finished. The second still
//! contains a parameter whose value depends on the environment, and the shell
//! must remember two things about it that a `String` cannot hold:
//!
//! * which parts came from an expansion, because only those are subject to
//!   field splitting; and
//! * whether the expansion was quoted, because `"$X"` is one field and `$X` may
//!   be several — or none.
//!
//! So a [`Word`] is a list of parts, and turning it into actual arguments is a
//! separate step that happens in the executor.
//!
//! # Why the parser does not expand
//!
//! Reading `$HOME` means reading the process environment, and this crate's one
//! architectural rule is that parsing never touches the operating system. A
//! parser that expanded would need a live environment to be tested at all.
//!
//! Splitting it this way means the *syntax* of expansion is tested here with no
//! environment, and the *semantics* are tested in the executor against a fake
//! one. Neither test needs a real process.

use crate::span::Span;

/// A parameter the shell substitutes at expansion time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parameter {
    /// A named variable: `$HOME`, `${HOME}`.
    Named(String),
    /// `$?` — the exit status of the previous command.
    Status,
    /// `$$` — the process id of the shell.
    Pid,
}

/// One piece of a word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordPart {
    /// Text that is already final.
    ///
    /// Quote removal and escape processing have happened, so this is never
    /// split further — which is what makes `echo a\ b` a single argument even
    /// though it contains a space.
    Literal(String),

    /// A parameter to substitute.
    Parameter {
        /// Which parameter.
        param: Parameter,
        /// Whether it appeared inside double quotes.
        ///
        /// This single flag is the whole difference between `$X` and `"$X"`:
        /// an unquoted expansion is split into fields on `IFS` and vanishes
        /// entirely when empty, while a quoted one is always exactly one
        /// field, even if that field is the empty string.
        quoted: bool,
    },

    /// A leading `~`, standing for the home directory.
    Tilde,
}

/// A single word of a command: a program name, an argument, or the target of a
/// redirection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    parts: Vec<WordPart>,
    span: Span,
    quoted: bool,
}

impl Word {
    /// Assemble a word from its parts.
    ///
    /// `quoted` records whether any part of the word was written inside quotes,
    /// anywhere. It is what makes `$X""` produce an empty argument when `X` is
    /// unset, while a bare `$X` produces no argument at all.
    pub fn new(parts: Vec<WordPart>, span: Span, quoted: bool) -> Self {
        Self {
            parts,
            span,
            quoted,
        }
    }

    /// Whether any part of the word was quoted or escaped.
    ///
    /// The executor uses this as a last resort: if expansion yields no fields
    /// at all but the user wrote quotes, the word still produces one empty
    /// argument, because they asked for it explicitly.
    pub fn has_quotes(&self) -> bool {
        self.quoted
    }

    /// The pieces, in order.
    pub fn parts(&self) -> &[WordPart] {
        &self.parts
    }

    /// Where the word appeared in the input.
    pub fn span(&self) -> Span {
        self.span
    }

    /// The word's text, if it needs no expansion at all.
    ///
    /// Most words in most commands are plain literals, and the executor uses
    /// this to skip the expansion machinery entirely for them.
    pub fn as_literal(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [WordPart::Literal(text)] => Some(text),
            _ => None,
        }
    }

    /// Whether anything here needs the environment to resolve.
    pub fn needs_expansion(&self) -> bool {
        self.parts
            .iter()
            .any(|part| matches!(part, WordPart::Parameter { .. } | WordPart::Tilde))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal(text: &str) -> Word {
        Word::new(
            vec![WordPart::Literal(text.into())],
            Span::new(0, text.len()),
            false,
        )
    }

    #[test]
    fn a_plain_word_reports_its_text_and_needs_nothing() {
        let word = literal("echo");
        assert_eq!(word.as_literal(), Some("echo"));
        assert!(!word.needs_expansion());
    }

    #[test]
    fn a_word_with_a_parameter_is_not_a_literal() {
        let word = Word::new(
            vec![
                WordPart::Literal("/home/".into()),
                WordPart::Parameter {
                    param: Parameter::Named("USER".into()),
                    quoted: false,
                },
            ],
            Span::new(0, 12),
            false,
        );
        assert_eq!(word.as_literal(), None);
        assert!(word.needs_expansion());
    }

    #[test]
    fn an_empty_literal_is_still_a_literal() {
        // `echo ""` must produce an argument, so the empty part has to survive
        // all the way to the executor.
        assert_eq!(literal("").as_literal(), Some(""));
    }
}
