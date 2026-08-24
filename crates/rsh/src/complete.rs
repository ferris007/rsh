//! Working out what a half-typed word might become.
//!
//! This lives in the binary rather than in `rsh-line`, because completing a
//! command means searching `PATH` and completing a path means reading
//! directories — neither belongs in a crate whose tests should run in
//! microseconds. `rsh-line` asks through a trait; this answers.
//!
//! # What is completed where
//!
//! The rule is positional, and it is the one every shell uses:
//!
//! | The word being typed | Completed as |
//! | --- | --- |
//! | first on the line | a command: builtins, then `PATH` |
//! | starts with `$` | an environment variable |
//! | contains a `/` | a path, even in first position |
//! | anything else | a path |
//!
//! `./scr` completing to a script rather than to a command on `PATH` is the
//! reason the `/` rule overrides position: a word with a slash in it is a path
//! by definition, which is the same rule `PATH` lookup follows.

use std::collections::BTreeSet;
use std::path::Path;

use rsh_line::{Completer, Completion};

/// Commands the shell implements itself, which no `PATH` search would find.
const BUILTINS: &[&str] = &["cd", "exit", "jobs", "fg", "bg"];

/// Completion against the real environment.
#[derive(Debug, Default)]
pub struct Shell;

impl Completer for Shell {
    fn complete(&self, line: &str, cursor: usize) -> Completion {
        let start = word_start(line, cursor);
        let word = &line[start..cursor];

        let candidates = if let Some(name) = word.strip_prefix('$') {
            variables(name)
        } else if word.contains('/') || !is_first_word(line, start) {
            paths(word)
        } else {
            commands(word)
        };

        Completion { start, candidates }
    }
}

/// Where the word under the cursor begins.
///
/// Split on whitespace only. Quoting is not honoured, so completing inside
/// `"some file`" treats the space as a boundary — a real limitation, and a
/// smaller one than it sounds, because the completion is still inserted at the
/// right place.
fn word_start(line: &str, cursor: usize) -> usize {
    line[..cursor]
        .rfind(char::is_whitespace)
        .map_or(0, |space| {
            space + line[space..].chars().next().map_or(1, char::len_utf8)
        })
}

/// Whether the word starting here is the command name.
fn is_first_word(line: &str, start: usize) -> bool {
    line[..start].trim().is_empty()
}

/// Executables on `PATH`, plus the builtins, that start with `prefix`.
///
/// A `BTreeSet` so the list comes out sorted and without the duplicates that a
/// `PATH` with overlapping directories produces — and a user does not care that
/// `ls` appears in two of them.
fn commands(prefix: &str) -> Vec<String> {
    let mut found: BTreeSet<String> = BUILTINS
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|name| (*name).to_owned())
        .collect();

    let Some(path) = std::env::var_os("PATH") else {
        return found.into_iter().collect();
    };

    for directory in std::env::split_paths(&path) {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(prefix) && is_executable(&entry.path()) {
                found.insert(name);
            }
        }
    }

    found.into_iter().collect()
}

/// Paths starting with `prefix`, with a `/` appended to directories.
///
/// The trailing slash is not decoration: it says the completion is not finished,
/// and it lets the next Tab continue into the directory without a keystroke in
/// between.
fn paths(prefix: &str) -> Vec<String> {
    let (directory, partial) = match prefix.rfind('/') {
        Some(slash) => (&prefix[..=slash], &prefix[slash + 1..]),
        None => ("", prefix),
    };

    let search = if directory.is_empty() { "." } else { directory };
    let Ok(entries) = std::fs::read_dir(search) else {
        return Vec::new();
    };

    let mut found: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();

            // Hidden files appear only when explicitly asked for. Otherwise a
            // Tab in a home directory buries the answer under dotfiles.
            if !name.starts_with(partial) || (name.starts_with('.') && !partial.starts_with('.')) {
                return None;
            }

            let suffix = if entry.path().is_dir() { "/" } else { "" };
            Some(format!("{directory}{name}{suffix}"))
        })
        .collect();

    found.sort();
    found
}

/// Environment variable names starting with `prefix`, keeping the `$`.
fn variables(prefix: &str) -> Vec<String> {
    let mut found: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| {
            let name = name.to_string_lossy().into_owned();
            name.starts_with(prefix).then(|| format!("${name}"))
        })
        .collect();

    found.sort();
    found
}

/// Whether a path is something that could be run.
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_word_completes_to_commands() {
        let found = Shell.complete("ex", 2);
        assert_eq!(found.start, 0);
        assert!(
            found.candidates.iter().any(|c| c == "exit"),
            "{:?}",
            found.candidates
        );
    }

    #[test]
    fn builtins_are_offered_even_though_path_has_never_heard_of_them() {
        // Asserted by membership, not by an exact list: what else is on PATH is
        // a property of the machine. This one has Windows directories on it.
        let found = Shell.complete("jo", 2);
        assert!(
            found.candidates.iter().any(|c| c == "jobs"),
            "{:?}",
            found.candidates
        );
    }

    #[test]
    fn candidates_come_back_sorted_and_deduplicated() {
        // A PATH with overlapping directories offers `ls` twice, and a user
        // does not care that it appears in two of them.
        let found = Shell.complete("l", 1);
        let mut sorted = found.candidates.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(found.candidates, sorted);
    }

    #[test]
    fn a_later_word_completes_to_paths() {
        let found = Shell.complete("cat /etc/hostn", 14);
        assert_eq!(found.start, 4);
        assert!(
            found.candidates.iter().any(|c| c.starts_with("/etc/hostn")),
            "{:?}",
            found.candidates
        );
    }

    #[test]
    fn a_word_with_a_slash_is_a_path_even_in_first_position() {
        // `./script` should find the script, not search PATH — the same rule
        // PATH lookup itself follows.
        let found = Shell.complete("/usr/bi", 7);
        assert_eq!(found.start, 0);
        assert!(
            found.candidates.iter().any(|c| c == "/usr/bin/"),
            "{:?}",
            found.candidates
        );
    }

    #[test]
    fn directories_are_offered_with_a_trailing_slash() {
        // Which says the completion is not finished, and lets the next Tab
        // continue without a keystroke in between.
        let found = Shell.complete("ls /us", 6);
        assert!(
            found.candidates.contains(&"/usr/".to_owned()),
            "{:?}",
            found.candidates
        );
    }

    #[test]
    fn a_dollar_completes_environment_variables() {
        std::env::set_var("RSH_COMPLETION_TEST", "1");
        let found = Shell.complete("echo $RSH_COMPLETION", 20);
        assert_eq!(found.start, 5);
        assert!(
            found
                .candidates
                .contains(&"$RSH_COMPLETION_TEST".to_owned()),
            "{:?}",
            found.candidates
        );
    }

    #[test]
    fn hidden_files_stay_hidden_until_asked_for() {
        // Otherwise a Tab in a home directory buries the answer under dotfiles.
        let visible = Shell.complete("ls /etc/", 8);
        assert!(
            !visible.candidates.iter().any(|c| c.starts_with("/etc/.")),
            "dotfiles should not appear unprompted"
        );
    }

    #[test]
    fn nothing_matching_is_no_candidates_rather_than_an_error() {
        assert!(Shell
            .complete("zzzzz-not-a-command", 19)
            .candidates
            .is_empty());
    }
}
