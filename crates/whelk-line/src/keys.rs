//! Turning bytes into keystrokes.
//!
//! In raw mode a terminal delivers bytes, and a keypress may be one byte or
//! six. Arrow keys, Home, End, and Delete arrive as **escape sequences** — an
//! `ESC`, usually a `[`, then some parameters and a final letter. There is no
//! length prefix and no framing; the only way to know a sequence has ended is
//! to recognise its last byte.
//!
//! This is why pressing Escape in a terminal program feels laggy: `ESC` alone
//! and `ESC` starting a sequence are the same first byte, and the program has
//! to either wait or guess.
//!
//! The decoder here takes the third option: it is fed a buffer and reports how
//! much it consumed, so an incomplete sequence simply stays in the buffer until
//! more bytes arrive. No timers, no guessing.

/// Something the user pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// An ordinary character.
    Char(char),
    /// Enter.
    Enter,
    /// Backspace, or Ctrl-H.
    Backspace,
    /// The Delete key, which removes forwards.
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Tab,
    /// Ctrl with a letter, normalised to lowercase.
    Control(char),
    /// Escape pressed on its own.
    Escape,
}

/// How much of the buffer a decode consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoded {
    /// A complete keystroke, and the bytes it used.
    Key(Key, usize),
    /// The buffer holds the start of a sequence but not all of it.
    Incomplete,
    /// The buffer is empty.
    Empty,
}

/// Read one keystroke from the front of a byte buffer.
pub fn decode(bytes: &[u8]) -> Decoded {
    let Some(&first) = bytes.first() else {
        return Decoded::Empty;
    };

    match first {
        0x1b => escape(bytes),
        b'\r' | b'\n' => Decoded::Key(Key::Enter, 1),
        b'\t' => Decoded::Key(Key::Tab, 1),
        // Both are "backspace" in practice: terminals disagree about which one
        // the key sends, and every program treats them alike.
        0x7f | 0x08 => Decoded::Key(Key::Backspace, 1),
        // Control characters are the letter with the top three bits cleared,
        // which is why Ctrl-A is 1 and Ctrl-Z is 26.
        byte @ 0x01..=0x1a => Decoded::Key(Key::Control((byte + b'a' - 1) as char), 1),
        byte if byte < 0x20 => Decoded::Key(Key::Control((byte + b'a' - 1) as char), 1),
        _ => character(bytes),
    }
}

/// Decode a UTF-8 character, which may span several bytes.
fn character(bytes: &[u8]) -> Decoded {
    let width = utf8_width(bytes[0]);

    if bytes.len() < width {
        return Decoded::Incomplete;
    }

    match std::str::from_utf8(&bytes[..width]) {
        Ok(text) => match text.chars().next() {
            Some(c) => Decoded::Key(Key::Char(c), width),
            None => Decoded::Key(Key::Char('\u{fffd}'), width),
        },
        // Not valid UTF-8. Consuming one byte keeps the editor moving rather
        // than wedging on input it cannot interpret.
        Err(_) => Decoded::Key(Key::Char('\u{fffd}'), 1),
    }
}

/// How many bytes a UTF-8 sequence starting with this byte occupies.
fn utf8_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        // A continuation byte with no lead. Treat it as one byte and move on.
        _ => 1,
    }
}

/// Decode an escape sequence.
fn escape(bytes: &[u8]) -> Decoded {
    match bytes.get(1) {
        // `ESC` on its own, so far. The caller decides whether more is coming;
        // a lone Escape is only knowable once input stops arriving.
        None => Decoded::Incomplete,

        // CSI — the common case: `ESC [ ... final`.
        Some(b'[') => csi(bytes),

        // SS3, used by some terminals for arrows in application mode.
        Some(b'O') => match bytes.get(2) {
            None => Decoded::Incomplete,
            Some(b'A') => Decoded::Key(Key::Up, 3),
            Some(b'B') => Decoded::Key(Key::Down, 3),
            Some(b'C') => Decoded::Key(Key::Right, 3),
            Some(b'D') => Decoded::Key(Key::Left, 3),
            Some(b'H') => Decoded::Key(Key::Home, 3),
            Some(b'F') => Decoded::Key(Key::End, 3),
            Some(_) => Decoded::Key(Key::Escape, 1),
        },

        // Escape followed by something else — Alt-key on many terminals.
        // Reported as a bare Escape, which the editor ignores; the character
        // will be decoded on the next pass.
        Some(_) => Decoded::Key(Key::Escape, 1),
    }
}

/// Decode a `ESC [ ...` sequence.
fn csi(bytes: &[u8]) -> Decoded {
    // Parameters are digits and semicolons; the sequence ends at the first byte
    // outside that set.
    let mut index = 2;
    while let Some(&byte) = bytes.get(index) {
        if byte.is_ascii_digit() || byte == b';' {
            index += 1;
            continue;
        }

        let used = index + 1;
        let key = match byte {
            b'A' => Key::Up,
            b'B' => Key::Down,
            b'C' => Key::Right,
            b'D' => Key::Left,
            b'H' => Key::Home,
            b'F' => Key::End,
            // `ESC [ n ~` — the number says which key.
            b'~' => match &bytes[2..index] {
                b"1" | b"7" => Key::Home,
                b"3" => Key::Delete,
                b"4" | b"8" => Key::End,
                _ => return Decoded::Key(Key::Escape, used),
            },
            _ => return Decoded::Key(Key::Escape, used),
        };

        return Decoded::Key(key, used);
    }

    Decoded::Incomplete
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(bytes: &[u8]) -> Decoded {
        decode(bytes)
    }

    #[test]
    fn ordinary_characters_are_one_byte() {
        assert_eq!(key(b"a"), Decoded::Key(Key::Char('a'), 1));
        assert_eq!(key(b"Z"), Decoded::Key(Key::Char('Z'), 1));
        assert_eq!(key(b" "), Decoded::Key(Key::Char(' '), 1));
    }

    #[test]
    fn multibyte_characters_are_decoded_whole() {
        assert_eq!(key("é".as_bytes()), Decoded::Key(Key::Char('é'), 2));
        assert_eq!(key("漢".as_bytes()), Decoded::Key(Key::Char('漢'), 3));
    }

    #[test]
    fn a_partial_character_waits_for_the_rest() {
        let full = "漢".as_bytes();
        assert_eq!(key(&full[..2]), Decoded::Incomplete);
    }

    #[test]
    fn control_characters_are_the_letter_with_bits_cleared() {
        // Ctrl-A is 1, which is why Ctrl-A means "start of line" everywhere.
        assert_eq!(key(&[0x01]), Decoded::Key(Key::Control('a'), 1));
        assert_eq!(key(&[0x03]), Decoded::Key(Key::Control('c'), 1));
        assert_eq!(key(&[0x1a]), Decoded::Key(Key::Control('z'), 1));
    }

    #[test]
    fn enter_and_backspace_have_more_than_one_spelling() {
        assert_eq!(key(b"\r"), Decoded::Key(Key::Enter, 1));
        assert_eq!(key(b"\n"), Decoded::Key(Key::Enter, 1));
        assert_eq!(key(&[0x7f]), Decoded::Key(Key::Backspace, 1));
        assert_eq!(key(&[0x08]), Decoded::Key(Key::Backspace, 1));
    }

    #[test]
    fn arrow_keys_are_escape_sequences() {
        assert_eq!(key(b"\x1b[A"), Decoded::Key(Key::Up, 3));
        assert_eq!(key(b"\x1b[B"), Decoded::Key(Key::Down, 3));
        assert_eq!(key(b"\x1b[C"), Decoded::Key(Key::Right, 3));
        assert_eq!(key(b"\x1b[D"), Decoded::Key(Key::Left, 3));
    }

    #[test]
    fn some_terminals_send_arrows_the_other_way() {
        assert_eq!(key(b"\x1bOA"), Decoded::Key(Key::Up, 3));
        assert_eq!(key(b"\x1bOD"), Decoded::Key(Key::Left, 3));
    }

    #[test]
    fn home_end_and_delete_have_numbered_forms() {
        assert_eq!(key(b"\x1b[H"), Decoded::Key(Key::Home, 3));
        assert_eq!(key(b"\x1b[F"), Decoded::Key(Key::End, 3));
        assert_eq!(key(b"\x1b[1~"), Decoded::Key(Key::Home, 4));
        assert_eq!(key(b"\x1b[3~"), Decoded::Key(Key::Delete, 4));
        assert_eq!(key(b"\x1b[4~"), Decoded::Key(Key::End, 4));
    }

    #[test]
    fn an_unfinished_sequence_stays_in_the_buffer() {
        // The whole reason the decoder reports how much it used: there is no
        // way to tell a lone Escape from the start of an arrow key except by
        // waiting, and waiting is the caller's decision.
        assert_eq!(key(b"\x1b"), Decoded::Incomplete);
        assert_eq!(key(b"\x1b["), Decoded::Incomplete);
        assert_eq!(key(b"\x1b[1"), Decoded::Incomplete);
    }

    #[test]
    fn an_empty_buffer_is_not_a_key() {
        assert_eq!(key(b""), Decoded::Empty);
    }

    #[test]
    fn an_unknown_sequence_is_consumed_rather_than_wedging() {
        // Better to swallow a keystroke nobody handles than to stop reading.
        assert_eq!(key(b"\x1b[5~"), Decoded::Key(Key::Escape, 4));
        assert_eq!(key(b"\x1b[Z"), Decoded::Key(Key::Escape, 3));
    }
}
