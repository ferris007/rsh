//! Turning words into arguments.
//!
//! The parser deliberately stops short of this: resolving `$HOME` means reading
//! the process environment, and a parser that did so could not be tested
//! without one. So expansion lives here, behind a trait, and the tests below
//! run against a `HashMap` rather than a real process.
//!
//! # Why a word is not an argument
//!
//! One word can become several arguments, or none at all:
//!
//! ```text
//! X="a b"     echo $X      →  echo a b        two arguments
//!             echo "$X"    →  echo "a b"      one argument
//! X unset     echo $X      →  echo            no argument at all
//!             echo "$X"    →  echo ""         one empty argument
//! ```
//!
//! That is field splitting, and the rule is narrow: only the *result of an
//! unquoted expansion* is split. Literal text is never split, which is why
//! `echo a\ b` stays a single argument even though it contains a space.

use std::collections::HashMap;

use rsh_parser::{Parameter, Word, WordPart};

/// The state expansion needs to resolve a parameter.
///
/// A trait rather than direct calls to [`std::env`] so that expansion can be
/// tested without touching the process — and so that Phase 2's shell variables,
/// when they arrive, have somewhere to live that is not a global.
pub trait Environment {
    /// The value of a variable, if it is set.
    fn var(&self, name: &str) -> Option<String>;

    /// The exit status of the previous command, for `$?`.
    fn last_status(&self) -> i32;

    /// The shell's process id, for `$$`.
    fn pid(&self) -> u32;

    /// Characters that separate fields when splitting an unquoted expansion.
    ///
    /// POSIX calls this `IFS`. The default — space, tab, newline — is what
    /// makes `for f in $(ls)` behave the way people expect, and the reason
    /// changing it is such an effective way to break a script.
    fn field_separators(&self) -> String {
        self.var("IFS").unwrap_or_else(|| " \t\n".to_owned())
    }
}

/// An [`Environment`] backed by the real process.
#[derive(Debug)]
pub struct ProcessEnv {
    last_status: i32,
}

impl ProcessEnv {
    /// Capture the shell state expansion is allowed to see.
    pub fn new(last_status: i32) -> Self {
        Self { last_status }
    }
}

impl Environment for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        // `var_os` rather than `var`: a variable that is set but not valid
        // UTF-8 is set, and reporting it as unset would be wrong. It cannot be
        // expanded into a Rust `String`, so it expands to nothing — but that is
        // a different fact, and worth not conflating.
        std::env::var_os(name)?.into_string().ok()
    }

    fn last_status(&self) -> i32 {
        self.last_status
    }

    fn pid(&self) -> u32 {
        std::process::id()
    }
}

/// An [`Environment`] built from a map, for tests.
#[derive(Debug, Default)]
pub struct MapEnv {
    vars: HashMap<String, String>,
    last_status: i32,
    pid: u32,
}

impl MapEnv {
    /// An empty environment.
    pub fn new() -> Self {
        Self {
            vars: HashMap::new(),
            last_status: 0,
            pid: 1234,
        }
    }

    /// Set a variable.
    #[must_use]
    pub fn with(mut self, name: &str, value: &str) -> Self {
        self.vars.insert(name.to_owned(), value.to_owned());
        self
    }

    /// Set the status `$?` reports.
    #[must_use]
    pub fn with_status(mut self, status: i32) -> Self {
        self.last_status = status;
        self
    }
}

impl Environment for MapEnv {
    fn var(&self, name: &str) -> Option<String> {
        self.vars.get(name).cloned()
    }

    fn last_status(&self) -> i32 {
        self.last_status
    }

    fn pid(&self) -> u32 {
        self.pid
    }
}

/// Expand a command's words into an argument vector.
pub fn expand_all(words: &[Word], env: &dyn Environment) -> Vec<String> {
    let separators = env.field_separators();
    let mut argv = Vec::with_capacity(words.len());
    for word in words {
        expand_into(word, env, &separators, &mut argv);
    }
    argv
}

/// Expand a single word, which must produce exactly one field.
///
/// Redirection targets work this way: `> $X` with `X="a b"` is an error in a
/// POSIX shell rather than a redirection to two files. `rsh` will need this in
/// Phase 3; it lives here so the rule is written down once.
pub fn expand_one(word: &Word, env: &dyn Environment) -> Result<String, AmbiguousRedirect> {
    let separators = env.field_separators();
    let mut fields = Vec::new();
    expand_into(word, env, &separators, &mut fields);

    match fields.len() {
        1 => Ok(fields.pop().expect("length checked")),
        _ => Err(AmbiguousRedirect {
            fields: fields.len(),
        }),
    }
}

/// A word that had to produce one field and did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmbiguousRedirect {
    /// How many fields it produced instead of one.
    pub fields: usize,
}

impl std::fmt::Display for AmbiguousRedirect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ambiguous redirect: expanded to {} words", self.fields)
    }
}

impl std::error::Error for AmbiguousRedirect {}

/// Expand one word, appending its fields to `out`.
fn expand_into(word: &Word, env: &dyn Environment, separators: &str, out: &mut Vec<String>) {
    let before = out.len();

    // `None` means "no field is open". The distinction from `Some("")` is the
    // whole of field splitting: an empty *open* field becomes an empty
    // argument, while no open field becomes no argument.
    let mut current: Option<String> = None;

    for part in word.parts() {
        match part {
            // Never split. Quote removal already happened, so a space here was
            // written as `\ ` or inside quotes and the user meant to keep it.
            WordPart::Literal(text) => current.get_or_insert_with(String::new).push_str(text),

            WordPart::Tilde => {
                // An unset HOME leaves the `~` alone. Substituting the empty
                // string would silently turn `~/notes` into `/notes`, which is
                // a real path somewhere, and probably not the user's.
                let home = env.var("HOME").unwrap_or_else(|| "~".to_owned());
                current.get_or_insert_with(String::new).push_str(&home);
            }

            WordPart::Parameter {
                param,
                quoted: true,
            } => {
                current
                    .get_or_insert_with(String::new)
                    .push_str(&resolve(param, env));
            }

            WordPart::Parameter {
                param,
                quoted: false,
            } => {
                for ch in resolve(param, env).chars() {
                    if separators.contains(ch) {
                        if let Some(field) = current.take() {
                            out.push(field);
                        }
                    } else {
                        current.get_or_insert_with(String::new).push(ch);
                    }
                }
            }
        }
    }

    if let Some(field) = current {
        out.push(field);
    }

    // The user wrote quotes but nothing survived — `""`, or `"$UNSET"`. They
    // asked for an argument explicitly, so they get an empty one.
    if out.len() == before && word.has_quotes() {
        out.push(String::new());
    }
}

/// The value a parameter stands for.
///
/// An unset variable expands to the empty string rather than an error. That is
/// POSIX behaviour, and the source of a famous class of accident — `rm -rf
/// "$PREFIX/"` with `PREFIX` unset. `set -u` is the guard against it, and it is
/// not implemented here yet.
fn resolve(param: &Parameter, env: &dyn Environment) -> String {
    match param {
        Parameter::Named(name) => env.var(name).unwrap_or_default(),
        Parameter::Status => env.last_status().to_string(),
        Parameter::Pid => env.pid().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsh_parser::parse;

    /// Expand a whole command line against a given environment.
    fn expand(input: &str, env: &dyn Environment) -> Vec<String> {
        let pipeline = parse(input)
            .expect("expected input to parse")
            .expect("expected a command");
        expand_all(pipeline.commands()[0].words(), env)
    }

    #[test]
    fn plain_words_pass_through() {
        assert_eq!(expand("echo hello", &MapEnv::new()), ["echo", "hello"]);
    }

    #[test]
    fn a_variable_is_substituted() {
        let env = MapEnv::new().with("NAME", "world");
        assert_eq!(expand("echo $NAME", &env), ["echo", "world"]);
        assert_eq!(expand("echo ${NAME}", &env), ["echo", "world"]);
    }

    #[test]
    fn braces_stop_the_name_early() {
        let env = MapEnv::new().with("X", "a");
        assert_eq!(expand("echo ${X}b", &env), ["echo", "ab"]);
        // Without braces the name would be `Xb`, which is unset — and an
        // unset unquoted variable produces no argument at all.
        assert_eq!(expand("echo $Xb", &env), ["echo"]);
    }

    #[test]
    fn an_unquoted_expansion_is_split_into_fields() {
        let env = MapEnv::new().with("LIST", "a b   c");
        assert_eq!(expand("echo $LIST", &env), ["echo", "a", "b", "c"]);
    }

    #[test]
    fn a_quoted_expansion_is_exactly_one_field() {
        let env = MapEnv::new().with("LIST", "a b   c");
        assert_eq!(expand(r#"echo "$LIST""#, &env), ["echo", "a b   c"]);
    }

    #[test]
    fn leading_and_trailing_separators_do_not_make_empty_fields() {
        let env = MapEnv::new().with("PADDED", "   a  b   ");
        assert_eq!(expand("echo $PADDED", &env), ["echo", "a", "b"]);
    }

    #[test]
    fn an_unset_variable_unquoted_produces_no_argument_at_all() {
        // This is the difference that bites people: `cmd $EMPTY` passes zero
        // arguments, `cmd "$EMPTY"` passes one.
        assert_eq!(expand("echo $NOPE", &MapEnv::new()), ["echo"]);
        assert_eq!(expand(r#"echo "$NOPE""#, &MapEnv::new()), ["echo", ""]);
    }

    #[test]
    fn empty_quotes_are_an_argument() {
        assert_eq!(expand(r#"echo "" x"#, &MapEnv::new()), ["echo", "", "x"]);
        assert_eq!(expand("echo '' x", &MapEnv::new()), ["echo", "", "x"]);
    }

    #[test]
    fn a_quoted_empty_tail_keeps_the_argument_alive() {
        // `$NOPE` alone would vanish; the quotes say "I want an argument here".
        assert_eq!(expand(r#"echo $NOPE"""#, &MapEnv::new()), ["echo", ""]);
    }

    #[test]
    fn literal_spaces_are_never_split() {
        // The word was escaped, so the space is data, not a separator.
        assert_eq!(expand(r"echo a\ b", &MapEnv::new()), ["echo", "a b"]);
    }

    #[test]
    fn adjacent_parts_concatenate() {
        let env = MapEnv::new().with("USER", "ferris");
        assert_eq!(
            expand("echo /home/$USER/src", &env),
            ["echo", "/home/ferris/src"]
        );
    }

    #[test]
    fn special_parameters_resolve() {
        let env = MapEnv::new().with_status(42);
        assert_eq!(expand("echo $?", &env), ["echo", "42"]);
        assert_eq!(expand("echo $$", &env), ["echo", "1234"]);
    }

    #[test]
    fn tilde_becomes_home() {
        let env = MapEnv::new().with("HOME", "/home/ferris");
        assert_eq!(expand("echo ~", &env), ["echo", "/home/ferris"]);
        assert_eq!(expand("echo ~/src", &env), ["echo", "/home/ferris/src"]);
    }

    #[test]
    fn tilde_without_home_stays_a_tilde() {
        // Expanding to nothing would turn `~/notes` into `/notes`, which is a
        // real path, and not the one anybody meant.
        assert_eq!(expand("echo ~/notes", &MapEnv::new()), ["echo", "~/notes"]);
    }

    #[test]
    fn a_custom_ifs_changes_the_split() {
        let env = MapEnv::new()
            .with("IFS", ":")
            .with("PATH_LIKE", "/bin:/usr/bin");
        assert_eq!(
            expand("echo $PATH_LIKE", &env),
            ["echo", "/bin", "/usr/bin"]
        );
    }

    #[test]
    fn a_redirect_target_must_be_a_single_word() {
        let env = MapEnv::new().with("TWO", "a b").with("ONE", "a");
        let pipeline = parse("cmd > $TWO").unwrap().unwrap();
        let target = pipeline.commands()[0].redirects()[0].kind().target();
        assert_eq!(
            expand_one(target, &env),
            Err(AmbiguousRedirect { fields: 2 })
        );

        let pipeline = parse("cmd > $ONE").unwrap().unwrap();
        let target = pipeline.commands()[0].redirects()[0].kind().target();
        assert_eq!(expand_one(target, &env), Ok("a".to_owned()));
    }
}
