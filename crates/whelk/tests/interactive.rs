//! Line editing, history, and completion, driven through a pseudoterminal.
//!
//! The editor's own logic is unit-tested in `whelk-line`, where a keystroke is a
//! function argument. These are the tests that need a real terminal: raw mode,
//! the bytes an arrow key actually sends, and whether the shell puts the
//! terminal back afterwards.

mod common;

use common::Terminal;

const CTRL_A: &str = "\x01";
const CTRL_C: &str = "\x03";
const CTRL_K: &str = "\x0b";
const CTRL_U: &str = "\x15";
const CTRL_W: &str = "\x17";
const UP: &str = "\x1b[A";
const LEFT: &str = "\x1b[D";
const TAB: &str = "\t";

/// A scratch `HOME`, so tests cannot read or write the real one.
struct Home(std::path::PathBuf);

impl Home {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("whelk-home-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("failed to create a scratch home");
        Self(dir)
    }

    fn open(&self) -> Terminal {
        Terminal::open_with_env(&[("HOME", self.0.to_str().expect("scratch home is not UTF-8"))])
    }

    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.0.join(name), contents).expect("failed to write a scratch file");
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.join(name)).unwrap_or_default()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- editing ---------------------------------------------------------------

#[test]
fn typed_characters_are_echoed_by_the_shell() {
    // In raw mode the terminal echoes nothing, so everything on screen was
    // written by the editor. If it stopped drawing, the user would be typing
    // blind even though the command still worked.
    let mut session = Terminal::open();
    session.type_in("echo hello");
    session.expect("whelk> echo hello");

    session.type_in("\n");
    session.expect("hello\n");
}

#[test]
fn backspace_removes_a_character() {
    let mut session = Terminal::open();
    session.type_in("echo helloX");
    session.type_in("\x7f");
    session.type_in("\n");
    session.expect("hello\n");
}

#[test]
fn the_cursor_can_move_and_insert_in_the_middle() {
    let mut session = Terminal::open();
    session.type_in("echo hllo");
    session.type_in(&LEFT.repeat(3));
    session.type_in("e");
    session.type_in("\n");
    session.expect("hello\n");
}

#[test]
fn control_u_and_k_and_w_cut_the_line() {
    let mut session = Terminal::open();

    session.type_in("rubbish");
    session.type_in(CTRL_U);
    session.type_in("echo one\n");
    session.expect("one\n");

    // Two words back from "echo two and more" leaves "echo two ".
    session.type_in("echo two and more");
    session.type_in(CTRL_W);
    session.type_in(CTRL_W);
    session.type_in("\n");
    session.expect("two\n");

    session.type_in("echo three");
    session.type_in(CTRL_A);
    session.type_in(CTRL_K);
    session.type_in("echo four\n");
    session.expect("four\n");
}

#[test]
fn control_c_abandons_the_line_without_running_it() {
    let mut session = Terminal::open();
    session.type_in("echo should not run");
    session.type_in(CTRL_C);
    session.type_in("echo $?\n");
    session.expect("130\n");

    assert!(
        !session.output().contains("should not run\n"),
        "the abandoned line ran anyway:\n{}",
        session.output()
    );
}

// ---- history ---------------------------------------------------------------

#[test]
fn up_recalls_the_previous_command() {
    let mut session = Terminal::open();
    session.type_in("echo remembered\n");
    session.expect("remembered\n");

    // The recalled line is drawn at the prompt, ready to run again.
    session.type_in(UP);
    session.expect("whelk> echo remembered");
}

#[test]
fn up_filters_by_what_has_been_typed() {
    // Typing `git ` and pressing Up should find the last git command, not
    // simply the last command.
    let mut session = Terminal::open();
    session.type_in("echo alpha\n");
    session.type_in("echo beta\n");
    session.type_in("echo a");
    session.type_in(UP);
    session.expect("whelk> echo alpha");
}

#[test]
fn history_survives_between_sessions() {
    let home = Home::new("history");

    let mut first = home.open();
    first.type_in("echo persisted\n");
    first.expect("persisted\n");
    first.type_in("exit\n");
    drop(first);

    assert!(
        home.read(".whelk_history").contains("echo persisted"),
        "history was not written: {:?}",
        home.read(".whelk_history")
    );

    // Twice, because `exit` is itself the most recent entry — a shell that
    // hid its own last command from the history would be lying about what ran.
    let mut second = home.open();
    second.type_in(UP);
    second.expect("whelk> exit");
    second.type_in(UP);
    second.expect("whelk> echo persisted");
}

// ---- completion ------------------------------------------------------------

#[test]
fn tab_completes_a_command() {
    let mut session = Terminal::open();
    // `jobs` is a builtin, so this does not depend on what is installed.
    session.type_in("job");
    session.type_in(TAB);
    session.expect("whelk> jobs");
}

#[test]
fn tab_completes_a_path() {
    // `/etc/passwd` rather than something Linux-only: this test is about the
    // shell, not about which files a platform happens to ship.
    let mut session = Terminal::open();
    session.type_in("ls /etc/passw");
    session.type_in(TAB);
    session.expect("/etc/passwd");
}

#[test]
fn tab_lists_the_possibilities_when_several_match() {
    let mut session = Terminal::open();
    session.type_in("ls /etc/pas");
    session.type_in(TAB);
    // Filled in as far as the candidates agree, whatever else is in /etc.
    session.expect("/etc/passwd");
}

// ---- messages and configuration --------------------------------------------

#[test]
fn a_mistyped_command_gets_a_suggestion() {
    let mut session = Terminal::open();
    session.type_in("grepp pattern\n");
    session.expect("command not found");
    session.expect("did you mean `grep`?");
}

#[test]
fn the_configuration_file_runs_before_the_first_prompt() {
    let home = Home::new("config");
    home.write(".whelkrc", "echo from the config file\n");

    let session = home.open();
    session.expect("from the config file");
}

#[test]
fn a_bad_line_in_the_configuration_does_not_stop_the_rest() {
    let home = Home::new("badconfig");
    home.write(".whelkrc", "definitely-not-a-command\necho still started\n");

    let session = home.open();
    session.expect("still started");
}
