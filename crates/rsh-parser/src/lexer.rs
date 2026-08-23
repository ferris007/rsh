//! Splitting a line into words.
//!
//! Phase 2 replaces this with a real lexer that emits tokens and performs
//! expansion. What is here now is the smallest thing that is not a lie: it
//! honours quoting and escaping, so words mean what the user meant, and it
//! refuses operators instead of swallowing them.

use std::iter::Peekable;
use std::str::CharIndices;

use crate::error::ParseError;

type Chars<'a> = Peekable<CharIndices<'a>>;

/// Split input into words, honouring quotes, escapes, and comments.
pub(crate) fn split(input: &str) -> Result<Vec<String>, ParseError> {
    let mut words = Vec::new();
    let mut word = String::new();
    // Tracked separately from `word.is_empty()`: `echo ""` produces a word
    // that is empty but real, and dropping it would change the command.
    let mut in_word = false;
    let mut chars = input.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }

            // A `#` only starts a comment at the beginning of a word, which is
            // why `curl example.com/#anchor` keeps its fragment.
            '#' if !in_word => break,

            '\'' => {
                in_word = true;
                single_quoted(&mut chars, &mut word, i)?;
            }

            '"' => {
                in_word = true;
                double_quoted(&mut chars, &mut word, i)?;
            }

            '\\' => match chars.next() {
                Some((_, escaped)) => {
                    in_word = true;
                    word.push(escaped);
                }
                // The escaped character is the newline, i.e. a request to
                // continue on the next line. `rsh` cannot read one yet.
                None => return Err(ParseError::TrailingBackslash { at: i }),
            },

            c if is_operator_start(c) => {
                let token = operator(&mut chars, c);
                return Err(ParseError::Unsupported {
                    phase: phase_for(&token),
                    token,
                    at: i,
                });
            }

            c => {
                in_word = true;
                word.push(c);
            }
        }
    }

    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// Consume a single-quoted string. Nothing inside is special — not even `\`.
fn single_quoted(chars: &mut Chars<'_>, word: &mut String, open: usize) -> Result<(), ParseError> {
    for (_, c) in chars.by_ref() {
        if c == '\'' {
            return Ok(());
        }
        word.push(c);
    }
    Err(ParseError::UnterminatedQuote {
        quote: '\'',
        at: open,
    })
}

/// Consume a double-quoted string.
///
/// Inside double quotes a backslash is only an escape before the characters
/// that are still special there; anywhere else it is a literal backslash. That
/// is why `"C:\path"` survives intact in a POSIX shell while `"\$HOME"` does
/// not print the backslash.
fn double_quoted(chars: &mut Chars<'_>, word: &mut String, open: usize) -> Result<(), ParseError> {
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Ok(()),
            '\\' => match chars.peek() {
                Some(&(_, next)) if matches!(next, '"' | '\\' | '$' | '`') => {
                    word.push(next);
                    chars.next();
                }
                Some(_) => word.push('\\'),
                None => return Err(ParseError::TrailingBackslash { at: i }),
            },
            c => word.push(c),
        }
    }
    Err(ParseError::UnterminatedQuote {
        quote: '"',
        at: open,
    })
}

/// Characters that begin a shell operator rather than a word.
fn is_operator_start(c: char) -> bool {
    matches!(c, '|' | '&' | ';' | '<' | '>' | '(' | ')')
}

/// Read a full operator, so the error can name `>>` rather than just `>`.
fn operator(chars: &mut Chars<'_>, first: char) -> String {
    let doubled = matches!(first, '|' | '&' | ';' | '<' | '>');
    if doubled {
        if let Some(&(_, next)) = chars.peek() {
            if next == first {
                chars.next();
                return format!("{first}{first}");
            }
        }
    }
    first.to_string()
}

/// The roadmap phase that will make an operator work.
///
/// Pointing at the phase turns "unsupported" into "not yet, and here is where
/// to look" — useful when the shell's own README is the spec.
fn phase_for(token: &str) -> u8 {
    match token {
        "<" | "<<" | ">" | ">>" => 3,
        "|" | "||" | "&&" => 4,
        "&" => 6,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(input: &str) -> Vec<String> {
        split(input).expect("expected input to lex")
    }

    #[test]
    fn splits_on_runs_of_whitespace() {
        assert_eq!(
            words("  echo   hello \tworld  "),
            ["echo", "hello", "world"]
        );
    }

    #[test]
    fn blank_input_yields_no_words() {
        assert!(words("").is_empty());
        assert!(words("   \t ").is_empty());
    }

    #[test]
    fn quotes_group_words() {
        assert_eq!(words(r#"echo "hello world""#), ["echo", "hello world"]);
        assert_eq!(words("echo 'hello world'"), ["echo", "hello world"]);
    }

    #[test]
    fn quotes_can_be_empty_and_still_count_as_arguments() {
        assert_eq!(words(r#"echo "" x"#), ["echo", "", "x"]);
    }

    #[test]
    fn quotes_can_open_and_close_mid_word() {
        assert_eq!(words(r#"echo a"b c"d"#), ["echo", "ab cd"]);
        assert_eq!(words("echo pre'fix'post"), ["echo", "prefixpost"]);
    }

    #[test]
    fn single_quotes_are_fully_literal() {
        assert_eq!(words(r#"echo '\n $HOME "x"'"#), ["echo", r#"\n $HOME "x""#]);
    }

    #[test]
    fn backslash_escapes_outside_quotes() {
        assert_eq!(words(r"echo hello\ world"), ["echo", "hello world"]);
        assert_eq!(words(r"echo \$HOME"), ["echo", "$HOME"]);
        assert_eq!(words(r"echo \>"), ["echo", ">"]);
    }

    #[test]
    fn backslash_inside_double_quotes_is_selective() {
        assert_eq!(words(r#"echo "a\$b""#), ["echo", "a$b"]);
        assert_eq!(words(r#"echo "a\"b""#), ["echo", r#"a"b"#]);
        // Not a special character there, so the backslash survives.
        assert_eq!(words(r#"echo "C:\path""#), ["echo", r"C:\path"]);
    }

    #[test]
    fn expansion_has_not_happened_yet() {
        // Phase 2 changes this. Asserting the current behaviour keeps the
        // change visible when it lands rather than silent.
        assert_eq!(words("echo $HOME"), ["echo", "$HOME"]);
    }

    #[test]
    fn comments_run_to_end_of_line() {
        assert_eq!(words("echo hi # a comment"), ["echo", "hi"]);
        assert!(words("# whole line").is_empty());
        assert_eq!(words("echo a#b"), ["echo", "a#b"]);
        assert_eq!(words(r##"echo "# quoted""##), ["echo", "# quoted"]);
    }

    #[test]
    fn unterminated_quotes_report_where_they_opened() {
        assert_eq!(
            split(r#"echo "oops"#),
            Err(ParseError::UnterminatedQuote { quote: '"', at: 5 })
        );
        assert_eq!(
            split("echo 'oops"),
            Err(ParseError::UnterminatedQuote { quote: '\'', at: 5 })
        );
    }

    #[test]
    fn trailing_backslash_is_a_line_continuation_we_cannot_honour() {
        assert_eq!(
            split(r"echo \"),
            Err(ParseError::TrailingBackslash { at: 5 })
        );
    }

    #[test]
    fn operators_are_refused_with_the_phase_that_implements_them() {
        assert_eq!(
            split("echo hi | grep hi"),
            Err(ParseError::Unsupported {
                token: "|".into(),
                phase: 4,
                at: 8
            })
        );
        assert_eq!(
            split("echo hi > out"),
            Err(ParseError::Unsupported {
                token: ">".into(),
                phase: 3,
                at: 8
            })
        );
        assert_eq!(
            split("echo hi >> out"),
            Err(ParseError::Unsupported {
                token: ">>".into(),
                phase: 3,
                at: 8
            })
        );
        assert_eq!(
            split("sleep 1 &"),
            Err(ParseError::Unsupported {
                token: "&".into(),
                phase: 6,
                at: 8
            })
        );
    }

    #[test]
    fn quoted_operators_are_ordinary_text() {
        assert_eq!(words(r#"echo "a | b""#), ["echo", "a | b"]);
        assert_eq!(words("echo '>'"), ["echo", ">"]);
    }
}
