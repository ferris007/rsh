//! Tokens to a syntax tree.
//!
//! The grammar is small enough to state in full:
//!
//! ```text
//! pipeline := command ( '|' command )*
//! command  := ( word | redirect )+          -- at least one word
//! redirect := [io_number] operator word
//! ```
//!
//! Everything else the lexer can produce — `&&`, `||`, `;`, `&`, `<<` — is
//! recognised and refused here, with the roadmap phase that will implement it.
//! Refusing at the parser rather than the lexer is the point of Phase 2: the
//! shell now understands the shape of what it is declining.

use crate::ast::{Command, Pipeline, Redirect, RedirectKind};
use crate::error::ParseError;
use crate::span::Span;
use crate::token::{Token, TokenKind};
use crate::word::Word;

/// Build a pipeline from a token stream.
///
/// Returns `Ok(None)` when there are no tokens — a blank line or a comment.
pub(crate) fn parse(tokens: Vec<Token>) -> Result<Option<Pipeline>, ParseError> {
    if tokens.is_empty() {
        return Ok(None);
    }
    Parser { tokens, pos: 0 }.pipeline().map(Some)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos).map(|token| &token.kind)
    }

    fn peek_token(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    /// The position just past the last token consumed, for pointing at things
    /// that are missing rather than wrong.
    fn end_of_previous(&self) -> usize {
        self.tokens
            .get(self.pos.saturating_sub(1))
            .map_or(0, |token| token.span.end)
    }

    // ---- grammar -----------------------------------------------------------

    fn pipeline(&mut self) -> Result<Pipeline, ParseError> {
        let mut commands = vec![self.command()?];
        let mut span = commands[0].span();

        while let Some(TokenKind::Pipe) = self.peek() {
            let pipe = self.bump().expect("peeked a pipe").span;

            // `echo hi |` with nothing after it. POSIX shells would prompt for
            // the rest of the pipeline; `rsh` has no continuation prompt, so it
            // points at the pipe and says what is missing.
            if self.peek().is_none() {
                return Err(ParseError::MissingCommand { span: pipe });
            }

            let command = self.command()?;
            span = span.to(command.span());
            commands.push(command);
        }

        // `&` ends the pipeline and sends it to the background. It is a
        // terminator, not a separator: `a & b` is two commands in bash, which
        // needs the list grammar this parser does not have yet.
        let background = matches!(self.peek(), Some(TokenKind::Amp));
        if background {
            let amp = self.bump().expect("peeked an ampersand").span;
            span = span.to(amp);
        }

        // Anything still unconsumed is a token the grammar has no place for.
        if let Some(token) = self.peek_token() {
            return Err(unsupported(token));
        }

        Ok(Pipeline::new(commands, background, span))
    }

    fn command(&mut self) -> Result<Command, ParseError> {
        let start = self.peek_token().map_or(0, |token| token.span.start);
        let mut words: Vec<Word> = Vec::new();
        let mut redirects: Vec<Redirect> = Vec::new();
        let mut end = start;

        loop {
            // Classified before doing anything, so the borrow of `self` from
            // `peek` is over by the time the arms need `&mut self`.
            let next = match self.peek() {
                Some(TokenKind::Word(_)) => Next::Word,
                Some(TokenKind::IoNumber(_)) => Next::Redirect,
                Some(kind) if kind.is_redirect_operator() => Next::Redirect,
                _ => break,
            };

            match next {
                Next::Word => {
                    let token = self.bump().expect("peeked a word");
                    end = token.span.end;
                    let TokenKind::Word(word) = token.kind else {
                        unreachable!("classified as a word")
                    };
                    words.push(word);
                }
                Next::Redirect => {
                    let redirect = self.redirect()?;
                    end = redirect.span().end;
                    redirects.push(redirect);
                }
            }
        }

        // A command made only of redirections — `> file` — is legal POSIX and
        // creates the file. `rsh` cannot run it usefully until Phase 3, and
        // guessing would be worse than saying so.
        if words.is_empty() {
            let span = if redirects.is_empty() {
                Span::empty(self.end_of_previous())
            } else {
                Span::new(start, end)
            };
            return Err(ParseError::MissingCommand { span });
        }

        Ok(Command::new(words, redirects, Span::new(start, end)))
    }

    fn redirect(&mut self) -> Result<Redirect, ParseError> {
        let start = self.peek_token().map_or(0, |token| token.span.start);

        // An explicit descriptor, as in `2>err`.
        let explicit_fd = match self.peek() {
            Some(&TokenKind::IoNumber(fd)) => {
                self.bump();
                Some(fd)
            }
            _ => None,
        };

        let operator = self.bump().ok_or(ParseError::MissingRedirectTarget {
            operator: "redirection".to_owned(),
            span: Span::empty(self.end_of_previous()),
        })?;

        // `<<` needs a here-document, which needs multi-line input.
        if operator.kind == TokenKind::DLess {
            return Err(ParseError::Unsupported {
                token: "<<".to_owned(),
                phase: None,
                span: operator.span,
            });
        }

        let target = match self.bump() {
            Some(Token {
                kind: TokenKind::Word(word),
                ..
            }) => word,
            // `echo >` or `echo > | grep`: the operator has nothing to act on.
            _ => {
                self.pos = self.pos.saturating_sub(1);
                return Err(ParseError::MissingRedirectTarget {
                    operator: operator.kind.describe().to_owned(),
                    span: operator.span,
                });
            }
        };

        let end = target.span().end;
        let kind = match operator.kind {
            TokenKind::Less => RedirectKind::Input(target),
            TokenKind::Great => RedirectKind::Output(target),
            TokenKind::DGreat => RedirectKind::Append(target),
            TokenKind::LessAnd => RedirectKind::DupInput(target),
            TokenKind::GreatAnd => RedirectKind::DupOutput(target),
            other => unreachable!("`{other}` is not a redirection operator"),
        };

        let fd = explicit_fd.unwrap_or_else(|| kind.default_fd());
        Ok(Redirect::new(fd, kind, Span::new(start, end)))
    }
}

/// What the next token contributes to a command.
#[derive(Debug, Clone, Copy)]
enum Next {
    Word,
    Redirect,
}

/// Turn a token the grammar cannot place into the most useful error available.
///
/// Where the roadmap promises the construct, the message names the phase. That
/// turns "unsupported" into "not yet, and here is where to look", which is the
/// difference between a dead end and a plan.
fn unsupported(token: &Token) -> ParseError {
    match token.kind {
        TokenKind::AndIf | TokenKind::OrIf | TokenKind::Semi | TokenKind::Amp => {
            ParseError::Unsupported {
                token: token.kind.describe().to_owned(),
                phase: None,
                span: token.span,
            }
        }
        _ => ParseError::UnexpectedToken {
            token: format!("`{}`", token.kind.describe()),
            span: token.span,
        },
    }
}
