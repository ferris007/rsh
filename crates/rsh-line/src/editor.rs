//! The line editor.
//!
//! A state machine: keys in, [`Action`] out, no I/O. See the crate docs for
//! why that shape was chosen.

use crate::history::History;
use crate::keys::Key;

/// What the caller should do after a keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The line changed, or did not. Redraw and keep reading.
    Continue,
    /// The user pressed Enter. Here is the line.
    Submit(String),
    /// Ctrl-C. The line is abandoned; the shell should report 130.
    Interrupted,
    /// Ctrl-D on an empty line.
    EndOfFile,
    /// Several completions matched. Show them, then redraw.
    Suggest(Vec<String>),
    /// Ctrl-L. Clear the screen, then redraw.
    Clear,
}

/// Somewhere to ask what a partial word might become.
///
/// A trait rather than a function so the editor stays free of the filesystem
/// and the environment: completing a command means searching `PATH`, and
/// completing a path means reading directories. Neither belongs in a component
/// whose tests should run in microseconds.
pub trait Completer {
    /// Offer completions for the word ending at `cursor`.
    fn complete(&self, line: &str, cursor: usize) -> Completion;
}

/// What a completer found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completion {
    /// Byte offset where the replacement starts — the beginning of the word.
    pub start: usize,
    /// The possibilities, in the order they should be shown.
    pub candidates: Vec<String>,
}

/// A completer that never has an opinion, for tests and non-interactive use.
#[derive(Debug, Default)]
pub struct NoCompletion;

impl Completer for NoCompletion {
    fn complete(&self, _line: &str, cursor: usize) -> Completion {
        Completion {
            start: cursor,
            candidates: Vec::new(),
        }
    }
}

/// A line being edited.
#[derive(Debug)]
pub struct Editor {
    buffer: String,
    /// Byte offset of the cursor. Always on a character boundary.
    cursor: usize,
    history: History,
    /// Where in the history the user has navigated to, if anywhere.
    browsing: Option<usize>,
    /// The line that was being typed before history navigation started.
    ///
    /// Held back so that pressing Up and then Down returns the half-finished
    /// command rather than an empty line — losing it is one of the most
    /// irritating things a shell can do.
    held: String,
    /// The prefix history navigation is filtering by.
    prefix: String,
}

impl Editor {
    /// A new editor over the given history.
    pub fn new(history: History) -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            history,
            browsing: None,
            held: String::new(),
            prefix: String::new(),
        }
    }

    /// The line as it currently reads.
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// Where the cursor is, as a byte offset.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The history, for saving.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Remember a submitted line.
    pub fn remember(&mut self, line: &str) {
        self.history.add(line);
    }

    /// Start a fresh line.
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.browsing = None;
        self.held.clear();
        self.prefix.clear();
    }

    /// Apply a keystroke.
    pub fn handle(&mut self, key: Key, completer: &dyn Completer) -> Action {
        // Any editing invalidates the history search: the prefix is whatever
        // was typed *before* the first Up, and typing changes it.
        let editing = matches!(
            key,
            Key::Char(_) | Key::Backspace | Key::Delete | Key::Control('u' | 'k' | 'w')
        );
        if editing {
            self.browsing = None;
        }

        match key {
            Key::Char(c) => {
                self.buffer.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                Action::Continue
            }

            Key::Enter => {
                let line = std::mem::take(&mut self.buffer);
                self.reset();
                Action::Submit(line)
            }

            Key::Backspace => {
                if let Some(previous) = self.previous_boundary() {
                    self.buffer.replace_range(previous..self.cursor, "");
                    self.cursor = previous;
                }
                Action::Continue
            }

            Key::Delete | Key::Control('d') if !self.buffer.is_empty() => {
                if let Some(next) = self.next_boundary() {
                    self.buffer.replace_range(self.cursor..next, "");
                }
                Action::Continue
            }

            // Ctrl-D on an empty line is end of input — the same keystroke
            // means "delete forwards" when there is something to delete, which
            // is a genuine ambiguity every shell resolves this way.
            Key::Control('d') => Action::EndOfFile,

            // Delete on an empty line has nothing to do.
            Key::Delete => Action::Continue,

            Key::Left => {
                if let Some(previous) = self.previous_boundary() {
                    self.cursor = previous;
                }
                Action::Continue
            }

            Key::Right => {
                if let Some(next) = self.next_boundary() {
                    self.cursor = next;
                }
                Action::Continue
            }

            Key::Home | Key::Control('a') => {
                self.cursor = 0;
                Action::Continue
            }

            Key::End | Key::Control('e') => {
                self.cursor = self.buffer.len();
                Action::Continue
            }

            Key::Up => self.browse_back(),
            Key::Down => self.browse_forward(),
            Key::Tab => self.complete(completer),

            Key::Control('c') => {
                self.reset();
                Action::Interrupted
            }

            Key::Control('l') => Action::Clear,

            // Kill to the start of the line.
            Key::Control('u') => {
                self.buffer.replace_range(..self.cursor, "");
                self.cursor = 0;
                Action::Continue
            }

            // Kill to the end of the line.
            Key::Control('k') => {
                self.buffer.truncate(self.cursor);
                Action::Continue
            }

            // Kill the word before the cursor.
            Key::Control('w') => {
                let start = self.word_start();
                self.buffer.replace_range(start..self.cursor, "");
                self.cursor = start;
                Action::Continue
            }

            Key::Control(_) | Key::Escape => Action::Continue,
        }
    }

    // ---- movement helpers --------------------------------------------------

    fn previous_boundary(&self) -> Option<usize> {
        self.buffer[..self.cursor]
            .chars()
            .next_back()
            .map(|c| self.cursor - c.len_utf8())
    }

    fn next_boundary(&self) -> Option<usize> {
        self.buffer[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
    }

    /// Where the word before the cursor begins.
    ///
    /// Trailing whitespace is skipped first, so Ctrl-W after `ls foo ` deletes
    /// `foo ` rather than nothing at all.
    fn word_start(&self) -> usize {
        let before = &self.buffer[..self.cursor];
        let trimmed = before.trim_end();
        match trimmed.rfind(char::is_whitespace) {
            Some(space) => space + 1,
            None => 0,
        }
    }

    // ---- history -----------------------------------------------------------

    fn browse_back(&mut self) -> Action {
        if self.browsing.is_none() {
            // Starting a search: hold the line being typed, and use it as the
            // prefix. Typing `git ` and pressing Up should find the last `git`
            // command, not simply the last command.
            self.held = self.buffer.clone();
            self.prefix = self.buffer[..self.cursor].to_owned();
        }

        let before = self.browsing.unwrap_or(self.history.len());
        let Some(found) = self.history.search_back(&self.prefix, before) else {
            return Action::Continue;
        };

        self.browsing = Some(found);
        self.show(self.history.get(found).unwrap_or_default().to_owned());
        Action::Continue
    }

    fn browse_forward(&mut self) -> Action {
        let Some(current) = self.browsing else {
            return Action::Continue;
        };

        match self.history.search_forward(&self.prefix, current) {
            Some(found) => {
                self.browsing = Some(found);
                self.show(self.history.get(found).unwrap_or_default().to_owned());
            }
            None => {
                // Past the newest match: back to what was being typed.
                self.browsing = None;
                let held = std::mem::take(&mut self.held);
                self.show(held);
            }
        }

        Action::Continue
    }

    /// Replace the line, putting the cursor at the end.
    fn show(&mut self, line: String) {
        self.buffer = line;
        self.cursor = self.buffer.len();
    }

    // ---- completion --------------------------------------------------------

    fn complete(&mut self, completer: &dyn Completer) -> Action {
        let found = completer.complete(&self.buffer, self.cursor);

        match found.candidates.len() {
            0 => Action::Continue,
            1 => {
                self.replace_word(found.start, &found.candidates[0]);
                Action::Continue
            }
            _ => {
                // Fill in as far as every candidate agrees, then show the list.
                // Completing to the common prefix is what makes repeated Tab
                // feel like progress rather than a menu.
                let shared = common_prefix(&found.candidates);
                if shared.len() > self.cursor - found.start {
                    self.replace_word(found.start, &shared);
                }
                Action::Suggest(found.candidates)
            }
        }
    }

    fn replace_word(&mut self, start: usize, replacement: &str) {
        self.buffer.replace_range(start..self.cursor, replacement);
        self.cursor = start + replacement.len();
    }
}

/// The longest prefix every candidate shares.
fn common_prefix(candidates: &[String]) -> String {
    let Some(first) = candidates.first() else {
        return String::new();
    };

    let mut length = first.len();
    for candidate in &candidates[1..] {
        length = length.min(
            first
                .char_indices()
                .zip(candidate.char_indices())
                .take_while(|((_, a), (_, b))| a == b)
                .map(|((i, c), _)| i + c.len_utf8())
                .last()
                .unwrap_or(0),
        );
    }

    first[..length].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An editor with the given history, oldest first.
    fn editor(history: &[&str]) -> Editor {
        let mut past = History::new(100);
        for line in history {
            past.add(line);
        }
        Editor::new(past)
    }

    /// Type a sequence of keys, returning the last action.
    fn press(editor: &mut Editor, keys: &[Key]) -> Action {
        let mut last = Action::Continue;
        for key in keys {
            last = editor.handle(*key, &NoCompletion);
        }
        last
    }

    /// Type a string of ordinary characters.
    fn type_in(editor: &mut Editor, text: &str) {
        for c in text.chars() {
            editor.handle(Key::Char(c), &NoCompletion);
        }
    }

    // ---- typing and movement -----------------------------------------------

    #[test]
    fn characters_land_where_the_cursor_is() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "helo");
        press(&mut editor, &[Key::Left]);
        type_in(&mut editor, "l");
        assert_eq!(editor.buffer(), "hello");
    }

    #[test]
    fn backspace_removes_the_character_before_the_cursor() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "hello");
        press(&mut editor, &[Key::Backspace]);
        assert_eq!(editor.buffer(), "hell");
    }

    #[test]
    fn backspace_at_the_start_does_nothing() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "hi");
        press(&mut editor, &[Key::Home, Key::Backspace]);
        assert_eq!(editor.buffer(), "hi");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn delete_removes_forwards() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "hello");
        press(&mut editor, &[Key::Home, Key::Delete]);
        assert_eq!(editor.buffer(), "ello");
    }

    #[test]
    fn multibyte_characters_move_and_delete_as_one() {
        // The cursor is a byte offset, so anything that steps by one byte
        // would split a character and panic on the next slice.
        let mut editor = editor(&[]);
        type_in(&mut editor, "héllo");
        press(&mut editor, &[Key::Home, Key::Right, Key::Right]);
        assert_eq!(editor.cursor(), 3, "past h and é");
        press(&mut editor, &[Key::Backspace]);
        assert_eq!(editor.buffer(), "hllo");
    }

    #[test]
    fn home_and_end_go_to_the_ends() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "hello");
        press(&mut editor, &[Key::Home]);
        assert_eq!(editor.cursor(), 0);
        press(&mut editor, &[Key::End]);
        assert_eq!(editor.cursor(), 5);
    }

    #[test]
    fn control_a_and_e_are_home_and_end() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "hello");
        press(&mut editor, &[Key::Control('a')]);
        assert_eq!(editor.cursor(), 0);
        press(&mut editor, &[Key::Control('e')]);
        assert_eq!(editor.cursor(), 5);
    }

    // ---- killing -----------------------------------------------------------

    #[test]
    fn control_u_kills_to_the_start() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "some command");
        press(
            &mut editor,
            &[Key::Left, Key::Left, Key::Left, Key::Control('u')],
        );
        assert_eq!(editor.buffer(), "and");
    }

    #[test]
    fn control_k_kills_to_the_end() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "some command");
        press(
            &mut editor,
            &[Key::Home, Key::Right, Key::Right, Key::Right, Key::Right],
        );
        press(&mut editor, &[Key::Control('k')]);
        assert_eq!(editor.buffer(), "some");
    }

    #[test]
    fn control_w_kills_the_previous_word() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "git commit --amend");
        press(&mut editor, &[Key::Control('w')]);
        assert_eq!(editor.buffer(), "git commit ");
    }

    #[test]
    fn control_w_skips_trailing_spaces_first() {
        // Otherwise Ctrl-W after `ls foo ` would delete nothing at all.
        let mut editor = editor(&[]);
        type_in(&mut editor, "ls foo   ");
        press(&mut editor, &[Key::Control('w')]);
        assert_eq!(editor.buffer(), "ls ");
    }

    // ---- submitting and leaving --------------------------------------------

    #[test]
    fn enter_submits_the_line_and_clears_it() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "echo hi");
        assert_eq!(
            press(&mut editor, &[Key::Enter]),
            Action::Submit("echo hi".into())
        );
        assert_eq!(editor.buffer(), "");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn control_c_abandons_the_line() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "half a command");
        assert_eq!(
            press(&mut editor, &[Key::Control('c')]),
            Action::Interrupted
        );
        assert_eq!(editor.buffer(), "");
    }

    #[test]
    fn control_d_ends_input_only_when_the_line_is_empty() {
        // The same keystroke means "delete forwards" when there is something to
        // delete. Every shell resolves the ambiguity this way.
        let mut editor = editor(&[]);
        assert_eq!(press(&mut editor, &[Key::Control('d')]), Action::EndOfFile);

        type_in(&mut editor, "hi");
        press(&mut editor, &[Key::Home]);
        assert_eq!(press(&mut editor, &[Key::Control('d')]), Action::Continue);
        assert_eq!(editor.buffer(), "i");
    }

    // ---- history -----------------------------------------------------------

    #[test]
    fn up_walks_backwards_through_history() {
        let mut editor = editor(&["first", "second"]);
        press(&mut editor, &[Key::Up]);
        assert_eq!(editor.buffer(), "second");
        press(&mut editor, &[Key::Up]);
        assert_eq!(editor.buffer(), "first");
        press(&mut editor, &[Key::Up]);
        assert_eq!(editor.buffer(), "first", "there is nothing older");
    }

    #[test]
    fn down_walks_back_towards_the_present() {
        let mut editor = editor(&["first", "second"]);
        press(&mut editor, &[Key::Up, Key::Up, Key::Down]);
        assert_eq!(editor.buffer(), "second");
    }

    #[test]
    fn a_half_typed_line_comes_back() {
        // Losing it is one of the most irritating things a shell can do.
        let mut editor = editor(&["old command"]);
        type_in(&mut editor, "old");
        press(&mut editor, &[Key::Up]);
        assert_eq!(editor.buffer(), "old command");
        press(&mut editor, &[Key::Down]);
        assert_eq!(editor.buffer(), "old", "the half-typed line should return");
    }

    #[test]
    fn up_finds_nothing_when_nothing_matches_the_prefix() {
        // A consequence of filtering, and the reason it is worth stating: Up on
        // a line that matches no history entry does nothing, where a shell with
        // unfiltered history would have jumped to the last command.
        let mut editor = editor(&["old command"]);
        type_in(&mut editor, "zzz");
        press(&mut editor, &[Key::Up]);
        assert_eq!(editor.buffer(), "zzz");
    }

    #[test]
    fn up_on_an_empty_line_is_plain_history() {
        // The empty prefix matches everything, so the filtered search and the
        // familiar behaviour are the same thing.
        let mut editor = editor(&["one", "two"]);
        press(&mut editor, &[Key::Up]);
        assert_eq!(editor.buffer(), "two");
    }

    #[test]
    fn up_filters_by_what_was_already_typed() {
        // Typing `git ` and pressing Up should find the last git command, not
        // simply the last command.
        let mut editor = editor(&["git status", "ls -la", "git commit"]);
        type_in(&mut editor, "git ");
        press(&mut editor, &[Key::Up]);
        assert_eq!(editor.buffer(), "git commit");
        press(&mut editor, &[Key::Up]);
        assert_eq!(editor.buffer(), "git status");
    }

    #[test]
    fn typing_ends_the_history_search() {
        let mut editor = editor(&["one", "two"]);
        press(&mut editor, &[Key::Up]);
        assert_eq!(editor.buffer(), "two");

        type_in(&mut editor, "!");
        press(&mut editor, &[Key::Down]);
        assert_eq!(
            editor.buffer(),
            "two!",
            "Down should no longer be navigating"
        );
    }

    #[test]
    fn down_with_no_search_in_progress_does_nothing() {
        let mut editor = editor(&["one"]);
        type_in(&mut editor, "typing");
        press(&mut editor, &[Key::Down]);
        assert_eq!(editor.buffer(), "typing");
    }

    // ---- completion --------------------------------------------------------

    struct Fixed(Vec<&'static str>);

    impl Completer for Fixed {
        fn complete(&self, line: &str, cursor: usize) -> Completion {
            let start = line[..cursor].rfind(' ').map_or(0, |space| space + 1);
            let word = &line[start..cursor];
            Completion {
                start,
                candidates: self
                    .0
                    .iter()
                    .filter(|candidate| candidate.starts_with(word))
                    .map(|candidate| (*candidate).to_owned())
                    .collect(),
            }
        }
    }

    #[test]
    fn a_single_match_is_filled_in() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "ec");
        let action = editor.handle(Key::Tab, &Fixed(vec!["echo"]));
        assert_eq!(action, Action::Continue);
        assert_eq!(editor.buffer(), "echo");
        assert_eq!(editor.cursor(), 4);
    }

    #[test]
    fn several_matches_fill_in_as_far_as_they_agree() {
        // Which is what makes repeated Tab feel like progress rather than a
        // menu that reappears unchanged.
        let mut editor = editor(&[]);
        type_in(&mut editor, "gi");
        let action = editor.handle(Key::Tab, &Fixed(vec!["gitk", "git-lfs"]));
        assert_eq!(editor.buffer(), "git");
        assert!(matches!(action, Action::Suggest(candidates) if candidates.len() == 2));
    }

    #[test]
    fn completion_applies_to_the_word_at_the_cursor() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "echo he");
        editor.handle(Key::Tab, &Fixed(vec!["hello"]));
        assert_eq!(editor.buffer(), "echo hello");
    }

    #[test]
    fn no_matches_leaves_the_line_alone() {
        let mut editor = editor(&[]);
        type_in(&mut editor, "zzz");
        assert_eq!(
            editor.handle(Key::Tab, &Fixed(vec!["echo"])),
            Action::Continue
        );
        assert_eq!(editor.buffer(), "zzz");
    }
}
