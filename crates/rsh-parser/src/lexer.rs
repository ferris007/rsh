//! Characters to tokens.
//!
//! The lexer does quote removal, escape processing, and the *recognition* of
//! parameter references — but never their evaluation. `$HOME` becomes a
//! [`WordPart::Parameter`], not a path. Reading the environment is the
//! executor's job; see [`crate::word`] for why the split is here.

use crate::error::ParseError;
use crate::span::Span;
use crate::token::{Token, TokenKind};
use crate::word::{Parameter, Word, WordPart};

/// Split a line into tokens.
pub(crate) fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    Lexer::new(input).run()
}

struct Lexer<'a> {
    source: &'a str,
    /// Every character with its byte offset, so the lexer can look ahead
    /// freely without re-walking the string.
    chars: Vec<(usize, char)>,
    /// Index into `chars`, not a byte offset.
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            chars: source.char_indices().collect(),
            pos: 0,
        }
    }

    // ---- cursor ------------------------------------------------------------

    fn peek(&self) -> Option<char> {
        self.peek_nth(0)
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).map(|&(_, c)| c)
    }

    /// Byte offset of the cursor, or the end of input.
    fn offset(&self) -> usize {
        self.chars
            .get(self.pos)
            .map_or(self.source.len(), |&(i, _)| i)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    // ---- driver ------------------------------------------------------------

    fn run(mut self) -> Result<Vec<Token>, ParseError> {
        let mut tokens = Vec::new();

        loop {
            while self.peek().is_some_and(|c| c.is_whitespace()) {
                self.pos += 1;
            }

            match self.peek() {
                None => break,
                // A `#` at the start of a token comments out the rest of the
                // line. Inside a word it is ordinary text, which is why
                // `curl example.com/#anchor` keeps its fragment.
                Some('#') => break,
                Some(c) if is_operator_start(c) => tokens.push(self.operator()?),
                Some(_) => tokens.push(self.word()?),
            }
        }

        Ok(tokens)
    }

    // ---- operators ---------------------------------------------------------

    fn operator(&mut self) -> Result<Token, ParseError> {
        let start = self.offset();
        let first = self.bump().expect("operator() called at end of input");

        let kind = match first {
            '|' if self.eat('|') => TokenKind::OrIf,
            '|' => TokenKind::Pipe,
            '&' if self.eat('&') => TokenKind::AndIf,
            '&' => TokenKind::Amp,
            ';' => TokenKind::Semi,
            '<' if self.eat('<') => TokenKind::DLess,
            '<' if self.eat('&') => TokenKind::LessAnd,
            '<' => TokenKind::Less,
            '>' if self.eat('>') => TokenKind::DGreat,
            '>' if self.eat('&') => TokenKind::GreatAnd,
            '>' => TokenKind::Great,
            // Subshells and command grouping. Recognised so they cannot be
            // mistaken for part of a word, but not on the roadmap yet.
            '(' | ')' => {
                return Err(ParseError::Unsupported {
                    token: first.to_string(),
                    phase: None,
                    span: Span::new(start, self.offset()),
                })
            }
            other => unreachable!("`{other}` is not an operator"),
        };

        Ok(Token::new(kind, Span::new(start, self.offset())))
    }

    // ---- words -------------------------------------------------------------

    fn word(&mut self) -> Result<Token, ParseError> {
        let start = self.offset();
        let mut parts: Vec<WordPart> = Vec::new();
        let mut literal = String::new();
        // Recorded because a file descriptor number must be *unquoted* digits:
        // `2>f` redirects stderr, `"2">f` writes the word `2` to a file.
        let mut had_quotes = false;

        while let Some(c) = self.peek() {
            if c.is_whitespace() || is_operator_start(c) {
                break;
            }

            match c {
                '\'' => {
                    had_quotes = true;
                    self.single_quoted(&mut literal)?;
                }

                '"' => {
                    had_quotes = true;
                    self.double_quoted(&mut parts, &mut literal)?;
                }

                '\\' => {
                    had_quotes = true;
                    let at = self.offset();
                    self.pos += 1;
                    match self.bump() {
                        Some(escaped) => literal.push(escaped),
                        // The escaped character is the newline: a request to
                        // continue on the next line, which needs multi-line
                        // input the shell does not have.
                        None => {
                            return Err(ParseError::TrailingBackslash {
                                span: Span::new(at, self.source.len()),
                            })
                        }
                    }
                }

                '$' => {
                    if let Some(part) = self.parameter(false)? {
                        if !literal.is_empty() {
                            parts.push(WordPart::Literal(std::mem::take(&mut literal)));
                        }
                        parts.push(part);
                    } else {
                        literal.push('$');
                    }
                }

                '`' => {
                    let at = self.offset();
                    return Err(ParseError::Unsupported {
                        token: "`".to_owned(),
                        phase: None,
                        span: Span::new(at, at + 1),
                    });
                }

                // A tilde only means "home" at the very start of an unquoted
                // word, and only when the word ends there or continues with a
                // `/`. Everywhere else it is an ordinary character, which is
                // why `echo a~b` prints `a~b`.
                '~' if parts.is_empty() && literal.is_empty() && !had_quotes => {
                    self.pos += 1;
                    if self
                        .peek()
                        .is_none_or(|c| c == '/' || c.is_whitespace() || is_operator_start(c))
                    {
                        parts.push(WordPart::Tilde);
                    } else {
                        literal.push('~');
                    }
                }

                _ => {
                    literal.push(c);
                    self.pos += 1;
                }
            }
        }

        // An empty word is still a word: `echo ""` passes one empty argument.
        // Nothing else can represent that, since a word with no parts is
        // indistinguishable from no word at all.
        if !literal.is_empty() || parts.is_empty() {
            parts.push(WordPart::Literal(literal));
        }

        let span = Span::new(start, self.offset());

        // POSIX: digits immediately followed by a redirection operator are a
        // file descriptor, not an argument. "Immediately" is doing real work
        // here — `echo 2 > f` and `echo 2> f` mean different things.
        if !had_quotes && self.peek().is_some_and(|c| c == '<' || c == '>') {
            if let [WordPart::Literal(text)] = parts.as_slice() {
                if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
                    if let Ok(fd) = text.parse::<i32>() {
                        return Ok(Token::new(TokenKind::IoNumber(fd), span));
                    }
                }
            }
        }

        Ok(Token::new(
            TokenKind::Word(Word::new(parts, span, had_quotes)),
            span,
        ))
    }

    /// Consume a single-quoted string. Nothing inside is special, not even `\`.
    fn single_quoted(&mut self, literal: &mut String) -> Result<(), ParseError> {
        let open = self.offset();
        self.pos += 1;

        while let Some(c) = self.bump() {
            if c == '\'' {
                return Ok(());
            }
            literal.push(c);
        }

        Err(ParseError::UnterminatedQuote {
            quote: '\'',
            span: Span::new(open, open + 1),
        })
    }

    /// Consume a double-quoted string, which still expands parameters.
    fn double_quoted(
        &mut self,
        parts: &mut Vec<WordPart>,
        literal: &mut String,
    ) -> Result<(), ParseError> {
        let open = self.offset();
        self.pos += 1;

        while let Some(c) = self.peek() {
            match c {
                '"' => {
                    self.pos += 1;
                    return Ok(());
                }

                // Inside double quotes a backslash escapes only the characters
                // that are still special there. Anywhere else it stays a
                // literal backslash, which is why `"C:\path"` survives intact.
                '\\' => {
                    let at = self.offset();
                    self.pos += 1;
                    match self.peek() {
                        Some(next) if matches!(next, '"' | '\\' | '$' | '`') => {
                            literal.push(next);
                            self.pos += 1;
                        }
                        Some(_) => literal.push('\\'),
                        None => {
                            return Err(ParseError::TrailingBackslash {
                                span: Span::new(at, self.source.len()),
                            })
                        }
                    }
                }

                '$' => {
                    if let Some(part) = self.parameter(true)? {
                        if !literal.is_empty() {
                            parts.push(WordPart::Literal(std::mem::take(literal)));
                        }
                        parts.push(part);
                    } else {
                        literal.push('$');
                    }
                }

                '`' => {
                    let at = self.offset();
                    return Err(ParseError::Unsupported {
                        token: "`".to_owned(),
                        phase: None,
                        span: Span::new(at, at + 1),
                    });
                }

                _ => {
                    literal.push(c);
                    self.pos += 1;
                }
            }
        }

        Err(ParseError::UnterminatedQuote {
            quote: '"',
            span: Span::new(open, open + 1),
        })
    }

    /// Read a parameter reference starting at `$`.
    ///
    /// Returns `Ok(None)` when the `$` is not introducing anything — a bare `$`
    /// at the end of a word, or before a character that cannot start a name.
    /// Every shell treats that as an ordinary dollar sign.
    fn parameter(&mut self, quoted: bool) -> Result<Option<WordPart>, ParseError> {
        let start = self.offset();
        self.pos += 1; // the `$`

        let param = match self.peek() {
            Some('{') => return self.braced_parameter(start, quoted).map(Some),
            Some('?') => {
                self.pos += 1;
                Parameter::Status
            }
            Some('$') => {
                self.pos += 1;
                Parameter::Pid
            }
            Some('(') => {
                return Err(ParseError::Unsupported {
                    token: "$(".to_owned(),
                    phase: None,
                    span: Span::new(start, start + 2),
                })
            }
            Some(c) if c.is_ascii_digit() => {
                self.pos += 1;
                return Err(ParseError::UnsupportedParameter {
                    text: format!("${c}"),
                    reason: "positional parameters are not supported",
                    span: Span::new(start, self.offset()),
                });
            }
            Some(c) if is_name_start(c) => Parameter::Named(self.name()),
            _ => return Ok(None),
        };

        Ok(Some(WordPart::Parameter { param, quoted }))
    }

    /// Read `${...}`.
    fn braced_parameter(&mut self, start: usize, quoted: bool) -> Result<WordPart, ParseError> {
        self.pos += 1; // the `{`

        let body_start = self.offset();
        while self.peek().is_some_and(|c| c != '}') {
            self.pos += 1;
        }

        if self.peek().is_none() {
            return Err(ParseError::UnterminatedBrace {
                span: Span::new(start, start + 2),
            });
        }

        let body = self.source[body_start..self.offset()].to_owned();
        self.pos += 1; // the `}`
        let span = Span::new(start, self.offset());

        let param = match body.as_str() {
            "?" => Parameter::Status,
            "$" => Parameter::Pid,
            name if is_name(name) => Parameter::Named(name.to_owned()),
            // `${X:-default}`, `${#X}`, `${X%.c}` and friends. The user wrote
            // something real; it is `rsh` that is incomplete, so the message
            // says so rather than calling it a syntax error.
            _ => {
                return Err(ParseError::UnsupportedParameter {
                    text: format!("${{{body}}}"),
                    reason: "only plain `${name}` is supported so far",
                    span,
                })
            }
        };

        Ok(WordPart::Parameter { param, quoted })
    }

    /// Read a bare variable name.
    fn name(&mut self) -> String {
        let start = self.offset();
        while self.peek().is_some_and(is_name_continue) {
            self.pos += 1;
        }
        self.source[start..self.offset()].to_owned()
    }
}

fn is_operator_start(c: char) -> bool {
    matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')')
}

fn is_name_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_name_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(is_name_start) && chars.all(is_name_continue)
}
