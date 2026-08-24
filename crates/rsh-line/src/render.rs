//! Drawing the line.
//!
//! In raw mode the terminal echoes nothing, so everything the user sees is
//! written by the editor. This module turns editor state into the bytes that
//! produce it.
//!
//! # Redraw, don't patch
//!
//! Every keystroke redraws the whole line: return to column 0, erase to the end
//! of the line, write the prompt and buffer, then move the cursor into place.
//!
//! Tracking what changed and patching it would be fewer bytes and considerably
//! more code — and every bug in it would be a line that looks wrong on screen
//! while the buffer is fine, which is the worst kind to diagnose. A terminal
//! redraw is a few dozen bytes over a link that carries megabytes.
//!
//! # What this deliberately does not handle
//!
//! A line longer than the terminal is wide. The escape sequences below address
//! a single row, so a wrapped line leaves fragments behind. Handling it
//! properly means knowing the terminal's width *and* how wide each character
//! is, and character width is a genuinely hard problem — East Asian characters
//! occupy two cells, combining marks occupy none, and emoji disagree with
//! everyone. Phase 8 stops short of it, and says so.

use crate::editor::Editor;

/// Move the cursor to the start of the line.
const TO_COLUMN_ZERO: &str = "\r";

/// Erase from the cursor to the end of the line.
const ERASE_TO_END: &str = "\x1b[K";

/// Clear the screen and move the cursor to the top-left.
pub const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";

/// The bytes that redraw the prompt and the line being edited.
pub fn line(prompt: &str, editor: &Editor) -> String {
    let mut out = String::with_capacity(prompt.len() + editor.buffer().len() + 16);

    out.push_str(TO_COLUMN_ZERO);
    out.push_str(ERASE_TO_END);
    out.push_str(prompt);
    out.push_str(editor.buffer());

    // Cursor position is counted in characters, not bytes: a byte offset would
    // put the cursor in the wrong column the moment the line contains anything
    // outside ASCII.
    let column = width(prompt) + width(&editor.buffer()[..editor.cursor()]);
    out.push_str(TO_COLUMN_ZERO);
    if column > 0 {
        out.push_str(&format!("\x1b[{column}C"));
    }

    out
}

/// How many columns a string occupies.
///
/// Counted in characters, which is right for ASCII and wrong for anything
/// double-width. See the note above: doing better needs a width table, and a
/// wrong width table is worse than a documented approximation.
fn width(text: &str) -> usize {
    text.chars().count()
}

/// The bytes that print a list of completions and then redraw the prompt.
///
/// Printed on lines of their own above a fresh prompt, which is the arrangement
/// every shell uses: the alternative is a menu that has to be erased, and
/// erasing it correctly needs the same width knowledge this module does not
/// have.
pub fn suggestions(prompt: &str, editor: &Editor, candidates: &[String]) -> String {
    let mut out = String::from("\r\n");

    for chunk in candidates.chunks(4) {
        for candidate in chunk {
            out.push_str(&format!("{candidate:<20}"));
        }
        out.push_str("\r\n");
    }

    out.push_str(&line(prompt, editor));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;
    use crate::keys::Key;
    use crate::NoCompletion;

    fn editor_with(text: &str, left: usize) -> Editor {
        let mut editor = Editor::new(History::new(10));
        for c in text.chars() {
            editor.handle(Key::Char(c), &NoCompletion);
        }
        for _ in 0..left {
            editor.handle(Key::Left, &NoCompletion);
        }
        editor
    }

    #[test]
    fn a_redraw_returns_erases_and_rewrites() {
        let drawn = line("rsh> ", &editor_with("echo hi", 0));
        assert!(
            drawn.starts_with("\r\x1b[K"),
            "should return and erase: {drawn:?}"
        );
        assert!(drawn.contains("rsh> echo hi"));
    }

    #[test]
    fn the_cursor_ends_up_after_the_prompt_and_the_text() {
        // 5 for "rsh> " plus 7 for "echo hi".
        let drawn = line("rsh> ", &editor_with("echo hi", 0));
        assert!(
            drawn.ends_with("\r\x1b[12C"),
            "cursor placement was {drawn:?}"
        );
    }

    #[test]
    fn a_cursor_moved_left_lands_earlier() {
        let drawn = line("rsh> ", &editor_with("echo hi", 2));
        assert!(
            drawn.ends_with("\r\x1b[10C"),
            "cursor placement was {drawn:?}"
        );
    }

    #[test]
    fn columns_are_characters_not_bytes() {
        // "héllo" is six bytes and five characters; a byte count would put the
        // cursor one column too far right.
        let drawn = line("", &editor_with("héllo", 0));
        assert!(
            drawn.ends_with("\r\x1b[5C"),
            "cursor placement was {drawn:?}"
        );
    }

    #[test]
    fn an_empty_line_with_no_prompt_needs_no_cursor_move() {
        let drawn = line("", &editor_with("", 0));
        assert!(
            !drawn.contains('C'),
            "should not emit a zero-column move: {drawn:?}"
        );
    }

    #[test]
    fn suggestions_appear_above_a_fresh_prompt() {
        let editor = editor_with("gi", 0);
        let drawn = suggestions("rsh> ", &editor, &["gitk".to_owned(), "git-lfs".to_owned()]);
        assert!(drawn.starts_with("\r\n"), "should move off the prompt line");
        assert!(drawn.contains("gitk"));
        assert!(drawn.contains("git-lfs"));
        assert!(
            drawn.contains("rsh> gi"),
            "should redraw the prompt underneath"
        );
    }
}
