//! Command history.

use std::path::Path;

/// Remembered command lines, oldest first.
#[derive(Debug)]
pub struct History {
    entries: Vec<String>,
    limit: usize,
}

impl History {
    /// An empty history holding at most `limit` entries.
    pub fn new(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit,
        }
    }

    /// Read history from a file, ignoring one that is missing or unreadable.
    ///
    /// A missing history file is the normal state of a new shell, and an
    /// unreadable one is not worth refusing to start over.
    pub fn load(path: &Path, limit: usize) -> Self {
        let mut history = Self::new(limit);

        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                history.add(line);
            }
        }

        history
    }

    /// Write history to a file, ignoring failure.
    ///
    /// Called on the way out, when there is nothing useful to do about an error
    /// and no terminal left to report it on.
    pub fn save(&self, path: &Path) {
        let text: String = self
            .entries
            .iter()
            .map(|line| format!("{line}\n"))
            .collect();
        let _ = std::fs::write(path, text);
    }

    /// Remember a line.
    ///
    /// Blank lines are dropped, and a line identical to the previous one is not
    /// stored twice — running the same command three times should not cost
    /// three presses of Up to get past.
    pub fn add(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }

        if self.entries.last().is_some_and(|last| last == line) {
            return;
        }

        self.entries.push(line.to_owned());

        if self.entries.len() > self.limit {
            let excess = self.entries.len() - self.limit;
            self.entries.drain(..excess);
        }
    }

    /// How many entries are remembered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// An entry by index, oldest first.
    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// Every entry, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(String::as_str)
    }

    /// The most recent entry starting with `prefix`, searching backwards from
    /// `before`.
    ///
    /// Used for prefix history search: typing `git ` and pressing Up should
    /// find the last `git` command rather than the last command.
    pub fn search_back(&self, prefix: &str, before: usize) -> Option<usize> {
        self.entries[..before.min(self.entries.len())]
            .iter()
            .rposition(|entry| entry.starts_with(prefix))
    }

    /// The oldest entry after `after` starting with `prefix`.
    pub fn search_forward(&self, prefix: &str, after: usize) -> Option<usize> {
        let start = after + 1;
        self.entries
            .get(start..)?
            .iter()
            .position(|entry| entry.starts_with(prefix))
            .map(|offset| start + offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history_of(lines: &[&str]) -> History {
        let mut history = History::new(100);
        for line in lines {
            history.add(line);
        }
        history
    }

    #[test]
    fn lines_are_remembered_in_order() {
        let history = history_of(&["one", "two"]);
        assert_eq!(history.get(0), Some("one"));
        assert_eq!(history.get(1), Some("two"));
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn blank_lines_are_not_worth_remembering() {
        let history = history_of(&["", "   ", "\t"]);
        assert!(history.is_empty());
    }

    #[test]
    fn a_repeated_line_is_stored_once() {
        // Running the same command three times should not cost three presses
        // of Up to get past it.
        let history = history_of(&["ls", "ls", "ls"]);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn a_repeat_that_is_not_consecutive_is_kept() {
        // `ls`, `cd`, `ls` is a real sequence, and losing the second `ls` would
        // change what Up means.
        let history = history_of(&["ls", "cd", "ls"]);
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn the_oldest_entries_fall_off_the_end() {
        let mut history = History::new(2);
        for line in ["one", "two", "three"] {
            history.add(line);
        }
        assert_eq!(history.iter().collect::<Vec<_>>(), ["two", "three"]);
    }

    #[test]
    fn searching_backwards_finds_the_most_recent_match() {
        let history = history_of(&["git status", "ls", "git commit"]);
        assert_eq!(history.search_back("git", 3), Some(2));
        // Continuing from there skips past the one already found.
        assert_eq!(history.search_back("git", 2), Some(0));
        assert_eq!(history.search_back("git", 0), None);
    }

    #[test]
    fn searching_forward_finds_the_next_match() {
        let history = history_of(&["git status", "ls", "git commit"]);
        assert_eq!(history.search_forward("git", 0), Some(2));
        assert_eq!(history.search_forward("git", 2), None);
    }

    #[test]
    fn an_empty_prefix_matches_everything() {
        // Which is what plain Up and Down are: a search for "".
        let history = history_of(&["one", "two"]);
        assert_eq!(history.search_back("", 2), Some(1));
        assert_eq!(history.search_forward("", 0), Some(1));
    }

    #[test]
    fn a_missing_file_is_an_empty_history_not_an_error() {
        let path = std::env::temp_dir().join("whelk-history-does-not-exist");
        let _ = std::fs::remove_file(&path);
        assert!(History::load(&path, 10).is_empty());
    }

    #[test]
    fn history_survives_a_round_trip_through_a_file() {
        let path = std::env::temp_dir().join(format!("whelk-history-{}", std::process::id()));
        history_of(&["one", "two"]).save(&path);

        let loaded = History::load(&path, 10);
        assert_eq!(loaded.iter().collect::<Vec<_>>(), ["one", "two"]);

        let _ = std::fs::remove_file(&path);
    }
}
