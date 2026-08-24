//! Parser tests.
//!
//! These use only the public API, which is the same surface the executor sees.
//! Nothing here needs an environment, a filesystem, or a process — that is the
//! property the crate exists to have.

use rsh_parser::{parse, tokenize, Parameter, ParseError, RedirectKind, WordPart};

/// The words of a single-command line, which must all be plain literals.
fn argv(input: &str) -> Vec<String> {
    let pipeline = parse(input)
        .expect("expected input to parse")
        .expect("expected a command");
    assert_eq!(
        pipeline.commands().len(),
        1,
        "expected one command in {input:?}"
    );
    pipeline.commands()[0]
        .words()
        .iter()
        .map(|word| {
            word.as_literal()
                .expect("expected a literal word")
                .to_owned()
        })
        .collect()
}

fn error(input: &str) -> ParseError {
    parse(input).expect_err("expected input to fail")
}

// ---- words and quoting -----------------------------------------------------

#[test]
fn splits_on_runs_of_whitespace() {
    assert_eq!(argv("  echo   hello \tworld  "), ["echo", "hello", "world"]);
}

#[test]
fn blank_input_has_no_command() {
    assert_eq!(parse(""), Ok(None));
    assert_eq!(parse("   \t "), Ok(None));
    assert_eq!(parse("# just a comment"), Ok(None));
}

#[test]
fn quotes_group_words() {
    assert_eq!(argv(r#"echo "hello world""#), ["echo", "hello world"]);
    assert_eq!(argv("echo 'hello world'"), ["echo", "hello world"]);
}

#[test]
fn quotes_can_open_and_close_mid_word() {
    assert_eq!(argv(r#"echo a"b c"d"#), ["echo", "ab cd"]);
    assert_eq!(argv("echo pre'fix'post"), ["echo", "prefixpost"]);
}

#[test]
fn single_quotes_are_fully_literal() {
    assert_eq!(argv(r#"echo '\n $HOME "x"'"#), ["echo", r#"\n $HOME "x""#]);
}

#[test]
fn backslash_escapes_outside_quotes() {
    assert_eq!(argv(r"echo hello\ world"), ["echo", "hello world"]);
    assert_eq!(argv(r"echo \$HOME"), ["echo", "$HOME"]);
    assert_eq!(argv(r"echo \>"), ["echo", ">"]);
}

#[test]
fn backslash_inside_double_quotes_is_selective() {
    assert_eq!(argv(r#"echo "a\$b""#), ["echo", "a$b"]);
    assert_eq!(argv(r#"echo "a\"b""#), ["echo", r#"a"b"#]);
    // Not special there, so the backslash survives.
    assert_eq!(argv(r#"echo "C:\path""#), ["echo", r"C:\path"]);
}

#[test]
fn comments_run_to_end_of_line_but_only_at_a_word_boundary() {
    assert_eq!(argv("echo hi # a comment"), ["echo", "hi"]);
    assert_eq!(argv("echo a#b"), ["echo", "a#b"]);
    assert_eq!(argv(r##"echo "# quoted""##), ["echo", "# quoted"]);
}

#[test]
fn quoted_operators_are_ordinary_text() {
    assert_eq!(argv(r#"echo "a | b""#), ["echo", "a | b"]);
    assert_eq!(argv("echo '>'"), ["echo", ">"]);
}

// ---- parameters ------------------------------------------------------------

/// The parts of the last word of a single command.
fn parts(input: &str) -> Vec<WordPart> {
    let pipeline = parse(input)
        .expect("expected input to parse")
        .expect("expected a command");
    let words = pipeline.commands()[0].words();
    words.last().expect("expected a word").parts().to_vec()
}

#[test]
fn a_bare_variable_is_recognised_but_not_resolved() {
    assert_eq!(
        parts("echo $HOME"),
        [WordPart::Parameter {
            param: Parameter::Named("HOME".into()),
            quoted: false
        }]
    );
}

#[test]
fn braces_delimit_a_name() {
    // The braces exist precisely so `${X}s` does not look up `Xs`.
    assert_eq!(
        parts("echo ${X}s"),
        [
            WordPart::Parameter {
                param: Parameter::Named("X".into()),
                quoted: false
            },
            WordPart::Literal("s".into()),
        ]
    );
}

#[test]
fn quoting_is_recorded_because_it_changes_field_splitting() {
    let quoted = parts(r#"echo "$X""#);
    assert_eq!(
        quoted,
        [WordPart::Parameter {
            param: Parameter::Named("X".into()),
            quoted: true
        }]
    );

    let bare = parts("echo $X");
    assert_eq!(
        bare,
        [WordPart::Parameter {
            param: Parameter::Named("X".into()),
            quoted: false
        }]
    );
}

#[test]
fn special_parameters_are_understood() {
    assert_eq!(
        parts("echo $?"),
        [WordPart::Parameter {
            param: Parameter::Status,
            quoted: false
        }]
    );
    assert_eq!(
        parts("echo $$"),
        [WordPart::Parameter {
            param: Parameter::Pid,
            quoted: false
        }]
    );
}

#[test]
fn a_dollar_that_starts_nothing_is_a_dollar() {
    assert_eq!(argv("echo $"), ["echo", "$"]);
    assert_eq!(argv("echo 5$ 6"), ["echo", "5$", "6"]);
}

#[test]
fn single_quotes_suppress_expansion_entirely() {
    assert_eq!(parts("echo '$HOME'"), [WordPart::Literal("$HOME".into())]);
}

#[test]
fn tilde_is_only_special_at_the_start_of_a_word() {
    assert_eq!(parts("echo ~"), [WordPart::Tilde]);
    assert_eq!(
        parts("echo ~/src"),
        [WordPart::Tilde, WordPart::Literal("/src".into())]
    );
    // Not at the start, so it is ordinary text.
    assert_eq!(parts("echo a~b"), [WordPart::Literal("a~b".into())]);
    assert_eq!(parts(r#"echo "~""#), [WordPart::Literal("~".into())]);
}

#[test]
fn unsupported_parameter_forms_say_so_rather_than_guessing() {
    assert!(matches!(
        error("echo ${X:-default}"),
        ParseError::UnsupportedParameter { .. }
    ));
    assert!(matches!(
        error("echo $1"),
        ParseError::UnsupportedParameter { .. }
    ));
    assert!(matches!(
        error("echo $(date)"),
        ParseError::Unsupported { .. }
    ));
    assert!(matches!(
        error("echo `date`"),
        ParseError::Unsupported { .. }
    ));
}

// ---- pipelines -------------------------------------------------------------

#[test]
fn a_single_command_is_a_pipeline_of_one() {
    let pipeline = parse("echo hi").unwrap().unwrap();
    assert_eq!(pipeline.commands().len(), 1);
}

#[test]
fn pipes_separate_commands() {
    let pipeline = parse("cat f | grep rust | sort").unwrap().unwrap();
    assert_eq!(pipeline.commands().len(), 3);
    assert_eq!(pipeline.commands()[1].words()[0].as_literal(), Some("grep"));
}

#[test]
fn a_pipe_needs_a_command_on_both_sides() {
    assert!(matches!(
        error("echo hi |"),
        ParseError::MissingCommand { .. }
    ));
    assert!(matches!(
        error("| grep hi"),
        ParseError::MissingCommand { .. }
    ));
}

// ---- redirections ----------------------------------------------------------

/// The single redirection of a single-command line.
fn redirect(input: &str) -> rsh_parser::Redirect {
    let pipeline = parse(input)
        .expect("expected input to parse")
        .expect("expected a command");
    pipeline.commands()[0].redirects()[0].clone()
}

#[test]
fn redirection_operators_are_parsed_with_their_default_descriptors() {
    let input = redirect("cat < in");
    assert_eq!(input.fd(), 0);
    assert!(matches!(input.kind(), RedirectKind::Input(_)));
    assert_eq!(input.kind().target().as_literal(), Some("in"));

    let output = redirect("echo hi > out");
    assert_eq!(output.fd(), 1);
    assert!(matches!(output.kind(), RedirectKind::Output(_)));
    assert_eq!(output.kind().target().as_literal(), Some("out"));

    let append = redirect("echo hi >> out");
    assert_eq!(append.fd(), 1);
    assert!(matches!(append.kind(), RedirectKind::Append(_)));
}

#[test]
fn an_explicit_descriptor_overrides_the_default() {
    let pipeline = parse("cmd 2> errors.txt").unwrap().unwrap();
    let redirect = &pipeline.commands()[0].redirects()[0];
    assert_eq!(redirect.fd(), 2);
    assert_eq!(redirect.kind().target().as_literal(), Some("errors.txt"));
}

#[test]
fn a_descriptor_number_must_touch_the_operator() {
    // `echo 2 > f` prints 2 and redirects stdout; `echo 2> f` prints nothing.
    let spaced = parse("echo 2 > f").unwrap().unwrap();
    assert_eq!(spaced.commands()[0].words().len(), 2);
    assert_eq!(spaced.commands()[0].redirects()[0].fd(), 1);

    let joined = parse("echo 2> f").unwrap().unwrap();
    assert_eq!(joined.commands()[0].words().len(), 1);
    assert_eq!(joined.commands()[0].redirects()[0].fd(), 2);
}

#[test]
fn quoted_digits_are_a_word_not_a_descriptor() {
    let pipeline = parse(r#"echo "2">f"#).unwrap().unwrap();
    assert_eq!(pipeline.commands()[0].words().len(), 2);
    assert_eq!(pipeline.commands()[0].redirects()[0].fd(), 1);
}

#[test]
fn descriptors_can_be_duplicated() {
    let pipeline = parse("cmd > out 2>&1").unwrap().unwrap();
    let redirects = pipeline.commands()[0].redirects();
    assert_eq!(redirects.len(), 2);
    assert_eq!(redirects[1].fd(), 2);
    assert!(matches!(redirects[1].kind(), RedirectKind::DupOutput(_)));
    assert_eq!(redirects[1].kind().target().as_literal(), Some("1"));
}

#[test]
fn redirection_order_is_preserved_because_it_changes_the_meaning() {
    let pipeline = parse("cmd 2>&1 > out").unwrap().unwrap();
    let redirects = pipeline.commands()[0].redirects();
    assert!(matches!(redirects[0].kind(), RedirectKind::DupOutput(_)));
    assert!(matches!(redirects[1].kind(), RedirectKind::Output(_)));
}

#[test]
fn redirections_can_appear_before_the_command_name() {
    let pipeline = parse("> out echo hi").unwrap().unwrap();
    let command = &pipeline.commands()[0];
    assert_eq!(command.words()[0].as_literal(), Some("echo"));
    assert_eq!(command.redirects().len(), 1);
}

#[test]
fn each_pipeline_stage_keeps_its_own_redirections() {
    let pipeline = parse("echo hello | grep hello > result.txt")
        .unwrap()
        .unwrap();
    assert!(pipeline.commands()[0].redirects().is_empty());
    assert_eq!(pipeline.commands()[1].redirects().len(), 1);
}

#[test]
fn a_dangling_operator_is_reported() {
    assert!(matches!(
        error("echo >"),
        ParseError::MissingRedirectTarget { .. }
    ));
    assert!(matches!(
        error("echo > | grep x"),
        ParseError::MissingRedirectTarget { .. }
    ));
}

// ---- errors ----------------------------------------------------------------

#[test]
fn errors_point_at_the_offending_characters() {
    let err = error(r#"echo "oops"#);
    assert!(matches!(
        err,
        ParseError::UnterminatedQuote { quote: '"', .. }
    ));
    assert_eq!(err.span().start, 5);

    let err = error(r"echo \");
    assert!(matches!(err, ParseError::TrailingBackslash { .. }));
    assert_eq!(err.span().start, 5);

    let err = error("echo ${X");
    assert!(matches!(err, ParseError::UnterminatedBrace { .. }));
    assert_eq!(err.span().start, 5);
}

#[test]
fn unimplemented_operators_are_refused_by_name() {
    for input in ["a && b", "a || b", "a ; b", "cat << EOF", "(a)"] {
        let err = error(input);
        assert!(
            matches!(err, ParseError::Unsupported { .. }),
            "{input} produced {err:?}"
        );
    }
}

#[test]
fn a_trailing_ampersand_marks_the_pipeline_as_background() {
    let pipeline = parse("sleep 5 &").unwrap().unwrap();
    assert!(pipeline.background());
    assert_eq!(
        pipeline.commands()[0].words()[0].as_literal(),
        Some("sleep")
    );

    // The `&` is a terminator, not an argument.
    assert_eq!(pipeline.commands()[0].words().len(), 2);
}

#[test]
fn a_whole_pipeline_can_go_to_the_background() {
    let pipeline = parse("cat f | grep x &").unwrap().unwrap();
    assert!(pipeline.background());
    assert_eq!(pipeline.commands().len(), 2);
}

#[test]
fn without_an_ampersand_a_pipeline_is_foreground() {
    assert!(!parse("sleep 5").unwrap().unwrap().background());
}

#[test]
fn an_ampersand_only_terminates_it_does_not_separate() {
    // `a & b` is two commands in bash, which needs a list grammar this parser
    // does not have. Refusing is better than silently running only `a`.
    assert!(matches!(
        error("sleep 5 & echo hi"),
        ParseError::UnexpectedToken { .. }
    ));
}

// ---- tokens ----------------------------------------------------------------

#[test]
fn tokenizing_is_available_on_its_own() {
    // The lexer is where a shell's surprises live, so it is worth being able to
    // look at directly.
    let tokens = tokenize("echo hi > out").unwrap();
    assert_eq!(tokens.len(), 4);
    assert_eq!(tokens[2].kind.describe(), ">");
    assert_eq!(tokens[2].span.slice("echo hi > out"), Some(">"));
}
