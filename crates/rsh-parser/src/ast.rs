//! The Phase 1 syntax tree.
//!
//! One node, because the language has one construct. Phases 2–4 grow this into
//! `Pipeline`/`Command`/`Redirection`; keeping it minimal now means the shape
//! is driven by what the executor actually needs rather than by a guess about
//! what it will need later.

/// A simple command: a program name and its arguments.
///
/// Guaranteed non-empty — a `Command` cannot exist without a program to run,
/// which is what lets the executor index `argv[0]` without a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    program: String,
    args: Vec<String>,
}

impl Command {
    /// Build a command from already-split words.
    ///
    /// Returns `None` if there are no words, so "blank line" is represented by
    /// the absence of a command rather than by a `Command` with an empty name.
    pub(crate) fn from_words(mut words: Vec<String>) -> Option<Self> {
        if words.is_empty() {
            return None;
        }
        let args = words.split_off(1);
        let program = words.pop()?;
        Some(Self { program, args })
    }

    /// The program to run, as written by the user.
    ///
    /// This is the name *before* `PATH` resolution: `ls`, not `/usr/bin/ls`.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// Arguments, excluding the program name.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The full argument vector, program name first.
    ///
    /// This is what `argv` means to `exec`: by convention `argv[0]` is the
    /// name the program was invoked as, which is why it is the user's spelling
    /// and not the resolved path. Programs like `busybox` and `vim`/`vi`
    /// genuinely change behaviour based on it.
    pub fn argv(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_not_a_command() {
        assert_eq!(Command::from_words(vec![]), None);
    }

    #[test]
    fn argv_leads_with_the_program_name() {
        let cmd = Command::from_words(vec!["echo".into(), "hi".into()]).unwrap();
        assert_eq!(cmd.argv().collect::<Vec<_>>(), ["echo", "hi"]);
        assert_eq!(cmd.program(), "echo");
        assert_eq!(cmd.args(), ["hi"]);
    }
}
