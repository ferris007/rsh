//! End-to-end tests.
//!
//! These drive the shell the way a user does: spawn the real binary, feed it
//! bytes on stdin, inspect what comes back. There are no test-only hooks into
//! the shell's internals — if a behaviour cannot be observed from outside the
//! process, it is not a behaviour this project claims to have.
//!
//! Everything involving the working directory or the environment lives here
//! rather than in a unit test, because those are process-wide state and a
//! multi-threaded test harness cannot safely assert against them.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

/// The binary built for this test run, provided by Cargo.
const RSH: &str = env!("CARGO_BIN_EXE_rsh");

/// Run a script through the shell and collect everything it produced.
///
/// Closing stdin after writing is what produces the end-of-input the REPL
/// treats as Ctrl-D, so scripts do not need a trailing `exit`.
fn run(script: &str) -> Session {
    run_with_env(script, &[])
}

/// Run a script with extra variables in the shell's environment.
///
/// Expansion reads the real process environment, so this is how a test gives it
/// something to find.
fn run_with_env(script: &str, vars: &[(&str, &str)]) -> Session {
    run_full(script, vars, Path::new(env!("CARGO_MANIFEST_DIR")))
}

fn run_full(script: &str, vars: &[(&str, &str)], dir: &Path) -> Session {
    let mut command = Command::new(RSH);
    for (name, value) in vars {
        command.env(name, value);
    }

    let mut child = command
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn rsh");

    child
        .stdin
        .take()
        .expect("stdin was not piped")
        .write_all(script.as_bytes())
        .expect("failed to write script to rsh");

    Session(
        child
            .wait_with_output()
            .expect("failed to collect rsh output"),
    )
}

struct Session(Output);

impl Session {
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.0.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.0.stderr).into_owned()
    }

    fn code(&self) -> i32 {
        self.0.status.code().expect("rsh was killed by a signal")
    }
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A scratch directory unique to this process and call, removed on drop.
///
/// Redirection tests write real files. Keeping them out of the source tree
/// means a failing test cannot leave the repository dirty.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rsh-repl-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("failed to create scratch dir");
        Self(dir)
    }

    /// Run a script with the shell's working directory set here, so that
    /// redirection targets are opened inside the scratch directory.
    fn run(&self, script: &str) -> Session {
        run_full(script, &[], &self.0)
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name))
            .unwrap_or_else(|error| panic!("failed to read {name}: {error}"))
    }

    fn write(&self, name: &str, contents: &str) {
        fs::write(self.0.join(name), contents).expect("failed to write scratch file");
    }

    fn exists(&self, name: &str) -> bool {
        self.0.join(name).exists()
    }

    /// An absolute path inside the scratch directory, for scripts that change
    /// the working directory before the redirection is opened.
    fn path(&self, name: &str) -> String {
        self.0
            .join(name)
            .to_str()
            .expect("scratch path is not UTF-8")
            .to_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// ---- running commands ------------------------------------------------------

#[test]
fn runs_an_external_command_and_shows_its_output() {
    let session = run("echo hello\n");
    assert_eq!(session.stdout(), "hello\n");
    assert_eq!(session.code(), 0);
}

#[test]
fn runs_several_commands_in_order() {
    let session = run("echo one\necho two\necho three\n");
    assert_eq!(session.stdout(), "one\ntwo\nthree\n");
}

#[test]
fn resolves_commands_through_path() {
    // `sh` is found only because PATH is searched; the same command with an
    // absolute path must reach the same program.
    assert_eq!(run("sh -c 'echo found'\n").stdout(), "found\n");
    assert_eq!(run("/bin/sh -c 'echo found'\n").stdout(), "found\n");
}

#[test]
fn argv_zero_is_the_name_the_user_typed() {
    // Not the resolved path: programs like busybox switch behaviour on it.
    assert_eq!(run("sh -c 'echo $0'\n").stdout(), "sh\n");
}

#[test]
fn a_final_line_without_a_newline_still_runs() {
    assert_eq!(run("echo hello").stdout(), "hello\n");
}

// ---- exit status -----------------------------------------------------------

#[test]
fn end_of_input_exits_with_the_last_status() {
    // Ctrl-D is not a failure: the shell leaves with whatever the last command
    // reported.
    assert_eq!(run("sh -c 'exit 0'\n").code(), 0);
    assert_eq!(run("sh -c 'exit 3'\n").code(), 3);
}

#[test]
fn a_command_killed_by_a_signal_reports_128_plus_the_signal() {
    // 143 = 128 + SIGTERM. Scripts in the wild test for exactly this.
    assert_eq!(run("sh -c 'kill -TERM $$'\n").code(), 143);
}

#[test]
fn an_unknown_command_reports_127() {
    let session = run("definitely-not-a-real-command\n");
    assert_eq!(session.code(), 127);
    assert!(
        session.stderr().contains("command not found"),
        "stderr was {:?}",
        session.stderr()
    );
    assert_eq!(session.stdout(), "", "a failed lookup should print nothing");
}

#[test]
fn a_file_that_is_not_executable_reports_126() {
    // Distinct from 127: the file is there, the permission is not.
    let session = run("./Cargo.toml\n");
    assert_eq!(session.code(), 126);
    assert!(
        session.stderr().contains("permission denied"),
        "stderr was {:?}",
        session.stderr()
    );
}

#[test]
fn a_failed_command_does_not_stop_the_shell() {
    let session = run("definitely-not-a-real-command\necho still here\n");
    assert_eq!(session.stdout(), "still here\n");
}

// ---- quoting ---------------------------------------------------------------

#[test]
fn quotes_hold_a_word_together() {
    assert_eq!(run("echo 'a  b'\n").stdout(), "a  b\n");
    assert_eq!(run("echo \"a  b\"\n").stdout(), "a  b\n");
}

#[test]
fn escapes_survive_to_the_command() {
    assert_eq!(run("echo a\\ b\n").stdout(), "a b\n");
}

#[test]
fn blank_lines_and_comments_do_nothing() {
    // Notably, they do not disturb the exit status.
    let session = run("sh -c 'exit 5'\n\n   \n# just a comment\n");
    assert_eq!(session.code(), 5);
    assert_eq!(session.stdout(), "");
}

// ---- parse errors ----------------------------------------------------------

#[test]
fn unimplemented_operators_are_refused_by_name() {
    let session = run("echo hi && echo bye\n");
    let stderr = session.stderr();
    assert!(stderr.contains("&&"), "stderr was {stderr:?}");
    // Refused, not partially run: `echo` must not have executed.
    assert_eq!(session.stdout(), "");
    assert_eq!(session.code(), 2);
}

#[test]
fn control_operators_are_refused_too() {
    for script in [
        "true && echo yes\n",
        "true || echo no\n",
        "echo a ; echo b\n",
    ] {
        let session = run(script);
        assert_eq!(session.stdout(), "", "{script:?} ran something");
        assert_eq!(session.code(), 2, "{script:?}");
    }
}

#[test]
fn refusals_underline_the_offending_characters() {
    let session = run("echo a ; echo b\n");
    let stderr = session.stderr();
    assert!(stderr.contains('^'), "stderr was {stderr:?}");
    assert!(stderr.contains(';'), "stderr was {stderr:?}");
}

#[test]
fn an_unterminated_quote_is_reported() {
    let session = run("echo 'oops\n");
    assert!(
        session.stderr().contains("unterminated"),
        "stderr was {:?}",
        session.stderr()
    );
    assert_eq!(session.code(), 2);
}

#[test]
fn a_parse_error_does_not_stop_the_shell() {
    let session = run("echo hi && echo bye\necho still here\n");
    assert_eq!(session.stdout(), "still here\n");
}

// ---- builtins --------------------------------------------------------------

#[test]
fn cd_changes_the_directory_for_later_commands() {
    // The real assertion: `pwd` is an external program, so the change has to
    // have happened in the shell's own process for the child to inherit it.
    assert_eq!(run("cd /usr\npwd\n").stdout(), "/usr\n");
}

#[test]
fn cd_with_no_argument_goes_home() {
    let home = std::env::var("HOME").expect("HOME must be set to run this test");
    assert_eq!(run("cd /usr\ncd\npwd\n").stdout(), format!("{home}\n"));
}

#[test]
fn cd_dash_returns_to_the_previous_directory_and_says_where() {
    // Two lines of output: the announcement from `cd -`, then `pwd`.
    assert_eq!(run("cd /usr\ncd /\ncd -\npwd\n").stdout(), "/usr\n/usr\n");
}

#[test]
fn cd_exports_pwd_to_children() {
    // A child that reads $PWD and a child that calls getcwd() must agree.
    assert_eq!(run("cd /usr\nsh -c 'echo $PWD'\n").stdout(), "/usr\n");
}

#[test]
fn cd_to_a_missing_directory_fails_without_moving() {
    let session = run("cd /usr\ncd /no/such/directory\npwd\n");
    assert_eq!(session.stdout(), "/usr\n");
    assert!(
        session.stderr().contains("cd:"),
        "stderr was {:?}",
        session.stderr()
    );

    // Checked in its own run: the `pwd` above succeeds and resets `$?`.
    assert_eq!(run("cd /no/such/directory\n").code(), 1);
}

#[test]
fn exit_ends_the_shell_immediately() {
    let session = run("echo before\nexit\necho after\n");
    assert_eq!(session.stdout(), "before\n");
}

#[test]
fn exit_takes_a_status() {
    assert_eq!(run("exit 42\n").code(), 42);
}

#[test]
fn exit_with_no_argument_carries_the_last_status() {
    assert_eq!(run("sh -c 'exit 9'\nexit\n").code(), 9);
}

#[test]
fn exit_status_is_truncated_to_a_byte_like_every_shell() {
    // The kernel only carries the low 8 bits; 300 & 0xff == 44.
    assert_eq!(run("exit 300\n").code(), 44);
}

#[test]
fn builtins_are_found_before_path() {
    // If `exit` were resolved through PATH it would be a child process, and
    // this shell would still be running afterwards.
    let session = run("exit 7\necho unreachable\n");
    assert_eq!(session.stdout(), "");
    assert_eq!(session.code(), 7);
}

// ---- pipelines -------------------------------------------------------------

#[test]
fn a_pipeline_connects_two_commands() {
    assert_eq!(run("echo hello | tr a-z A-Z\n").stdout(), "HELLO\n");
}

#[test]
fn a_pipeline_can_be_longer_than_two() {
    let session = run("printf '3\\n1\\n2\\n' | sort | head -2\n");
    assert_eq!(session.stdout(), "1\n2\n");
}

#[test]
fn the_pipeline_status_is_the_last_commands() {
    // POSIX, and the reason `grep -q x | true` succeeds whatever grep found.
    assert_eq!(run("sh -c 'exit 3' | sh -c 'exit 0'\n").code(), 0);
    assert_eq!(run("sh -c 'exit 0' | sh -c 'exit 7'\n").code(), 7);
}

#[test]
fn a_signal_in_the_last_stage_still_reports_128_plus_the_signal() {
    assert_eq!(run("echo hi | sh -c 'kill -TERM $$'\n").code(), 143);
}

#[test]
fn a_stage_that_does_not_exist_does_not_stop_the_others() {
    let session = run("definitely-not-a-command | cat\necho done\n");
    assert!(
        session.stderr().contains("command not found"),
        "stderr was {:?}",
        session.stderr()
    );
    // `cat` still ran, saw an immediate end-of-input, and exited cleanly.
    assert_eq!(session.stdout(), "done\n");
}

#[test]
fn a_failing_last_stage_sets_the_status() {
    assert_eq!(run("echo hi | definitely-not-a-command\n").code(), 127);
}

#[test]
fn the_reader_sees_end_of_input_when_the_writer_finishes() {
    // If any copy of the write end were still open — in the shell or in
    // another child — `cat` would block here and the test would hang.
    assert_eq!(run("echo done | cat\n").stdout(), "done\n");
}

#[test]
fn a_stage_that_exits_early_does_not_wedge_the_pipeline() {
    let session = run("printf 'a\\nb\\nc\\n' | head -1\n");
    assert_eq!(session.stdout(), "a\n");
}

#[test]
fn children_inherit_the_default_action_for_sigpipe() {
    // Rust sets SIGPIPE to SIG_IGN for its own process, and SIG_IGN survives
    // exec. Without an explicit reset in the child, every program `rsh` runs
    // would start with SIGPIPE ignored — so `yes | head` would never have its
    // writer killed. 141 = 128 + SIGPIPE. See experiments/pipes.
    assert_eq!(run("sh -c 'kill -PIPE $$'\n").code(), 141);
}

#[test]
fn a_redirection_overrides_the_pipe() {
    // POSIX applies redirections after the pipe is wired up, so the file wins
    // and the next stage reads an immediate end-of-input.
    let scratch = Scratch::new();
    let session = scratch.run("echo hi > f.txt | cat\n");
    assert_eq!(session.stdout(), "", "cat should have received nothing");
    assert_eq!(scratch.read("f.txt"), "hi\n");
}

#[test]
fn pipe_descriptors_do_not_leak_into_the_children() {
    // Every child inherits a copy of every pipe. Close-on-exec plus `dup2`
    // means each keeps only the ends it was given.
    let session = run("sh -c 'echo leaked >&3' | cat\n");
    assert_eq!(session.stdout(), "", "a pipe end leaked into the child");
    assert!(
        session
            .stderr()
            .to_lowercase()
            .contains("bad file descriptor"),
        "stderr was {:?}",
        session.stderr()
    );
}

#[test]
fn a_builtin_in_a_pipeline_is_refused_rather_than_run_in_the_shell() {
    // Running it here would let `cd x | cat` move the real shell, which no
    // other shell does. Doing it properly needs a subshell.
    let session = run("cd /usr | cat\npwd\n");
    assert_eq!(session.code(), 0);
    assert!(
        session.stderr().contains("builtin"),
        "stderr was {:?}",
        session.stderr()
    );
    assert_ne!(
        session.stdout().trim(),
        "/usr",
        "the shell changed directory anyway"
    );
}

#[test]
fn expansion_applies_to_every_stage() {
    let session = run_with_env("echo $WORD | tr a-z A-Z\n", &[("WORD", "shouted")]);
    assert_eq!(session.stdout(), "SHOUTED\n");
}

// ---- redirection -----------------------------------------------------------

#[test]
fn output_redirection_writes_to_a_file() {
    let scratch = Scratch::new();
    let session = scratch.run("echo hello > out.txt\n");
    assert_eq!(session.stdout(), "", "output reached the terminal too");
    assert_eq!(scratch.read("out.txt"), "hello\n");
}

#[test]
fn output_redirection_truncates_an_existing_file() {
    let scratch = Scratch::new();
    scratch.write("out.txt", "old contents that should be gone\n");
    scratch.run("echo new > out.txt\n");
    assert_eq!(scratch.read("out.txt"), "new\n");
}

#[test]
fn append_redirection_keeps_what_is_there() {
    let scratch = Scratch::new();
    scratch.run("echo one > log.txt\necho two >> log.txt\n");
    assert_eq!(scratch.read("log.txt"), "one\ntwo\n");
}

#[test]
fn input_redirection_feeds_a_command() {
    let scratch = Scratch::new();
    scratch.write("in.txt", "from the file\n");
    assert_eq!(scratch.run("cat < in.txt\n").stdout(), "from the file\n");
}

#[test]
fn stderr_can_be_redirected_on_its_own() {
    let scratch = Scratch::new();
    let session = scratch.run("sh -c 'echo out; echo err >&2' 2> err.txt\n");
    assert_eq!(session.stdout(), "out\n");
    assert_eq!(scratch.read("err.txt"), "err\n");
}

#[test]
fn both_streams_can_go_to_one_file() {
    let scratch = Scratch::new();
    let session = scratch.run("sh -c 'echo out; echo err >&2' > both.txt 2>&1\n");
    assert_eq!(session.stdout(), "");
    assert_eq!(session.stderr(), "");
    let contents = scratch.read("both.txt");
    assert!(contents.contains("out"), "contents were {contents:?}");
    assert!(contents.contains("err"), "contents were {contents:?}");
}

#[test]
fn redirection_order_changes_the_meaning() {
    // `2>&1 >f` points stderr at wherever stdout was *then* — the terminal —
    // and only afterwards moves stdout to the file. The famous gotcha, and
    // entirely explained by the dup2 calls running left to right.
    // `2>&1` copies descriptor 1 *as it is then* — the shell's stdout, which
    // in this harness is a pipe. Only afterwards does stdout move to the file.
    // So `err` arrives on the shell's stdout, not its stderr, which is the
    // clearest possible demonstration that the order is what matters.
    let scratch = Scratch::new();
    let session = scratch.run("sh -c 'echo out; echo err >&2' 2>&1 > only-out.txt\n");
    assert_eq!(scratch.read("only-out.txt"), "out\n");
    assert_eq!(session.stdout(), "err\n");
    assert_eq!(session.stderr(), "");
}

#[test]
fn an_arbitrary_descriptor_can_be_redirected() {
    let scratch = Scratch::new();
    scratch.run("sh -c 'echo three >&3' 3> three.txt\n");
    assert_eq!(scratch.read("three.txt"), "three\n");
}

#[test]
fn the_opened_file_is_not_leaked_into_the_child() {
    // The shell opens the target at the lowest free descriptor, which is 3.
    // Without close-on-exec the program would inherit it and `>&3` would
    // silently succeed. It must not: a program gets the descriptors the user
    // asked for and no accidental extras.
    let scratch = Scratch::new();
    let session = scratch.run("sh -c 'echo leaked >&3' > out.txt\n");
    assert_eq!(
        scratch.read("out.txt"),
        "",
        "descriptor 3 leaked into the child"
    );
    assert!(
        session
            .stderr()
            .to_lowercase()
            .contains("bad file descriptor"),
        "stderr was {:?}",
        session.stderr()
    );
}

#[test]
fn redirection_applies_to_builtins_too() {
    // A builtin runs inside the shell, so there is no child to arrange
    // descriptors for. The shell moves its own and puts them back.
    // The target is absolute because redirections are opened *before* the
    // builtin runs — at which point the shell is still in /usr.
    let scratch = Scratch::new();
    let where_txt = scratch.path("where.txt");
    let session = scratch.run(&format!(
        "cd /usr\ncd - > {where_txt}\necho still working\n"
    ));
    assert_eq!(session.stdout(), "still working\n");
    assert!(
        !scratch.read("where.txt").is_empty(),
        "the announcement was lost"
    );
}

#[test]
fn the_shell_keeps_its_own_descriptors() {
    // After redirecting a builtin the shell's stdout must be back where it
    // started, or every later command would write into the file.
    let scratch = Scratch::new();
    let session = scratch.run("cd . > swallow.txt\necho back on stdout\n");
    assert_eq!(session.stdout(), "back on stdout\n");
}

#[test]
fn a_failed_redirection_stops_the_command_from_running() {
    let scratch = Scratch::new();
    let session = scratch.run("echo hi < missing.txt\n");
    assert_eq!(session.stdout(), "", "the command ran anyway");
    assert_eq!(session.code(), 1);
    assert!(
        session.stderr().contains("No such file or directory"),
        "stderr was {:?}",
        session.stderr()
    );
}

#[test]
fn the_file_is_created_even_when_the_command_does_not_exist() {
    // Redirections are set up before the command is looked up. Every POSIX
    // shell behaves this way, and `> lockfile` depends on it.
    let scratch = Scratch::new();
    let session = scratch.run("definitely-not-a-command > made.txt\n");
    assert_eq!(session.code(), 127);
    assert!(scratch.exists("made.txt"));
    assert_eq!(scratch.read("made.txt"), "");
}

#[test]
fn a_redirection_target_must_expand_to_one_word() {
    let scratch = Scratch::new();
    let session = scratch.run("echo hi > $UNSET\n");
    assert_eq!(session.code(), 1);
    assert!(
        session.stderr().contains("ambiguous redirect"),
        "stderr was {:?}",
        session.stderr()
    );
}

#[test]
fn duplicating_a_closed_descriptor_is_refused() {
    // The number is absurd on purpose: a shell inherits whatever descriptors
    // its launcher left open, and on some CI runners that includes single
    // digits — so `2>&9` is not reliably an error at all.
    let scratch = Scratch::new();
    let session = scratch.run("echo hi 2>&1000000\n");
    assert_eq!(session.code(), 1);
    assert!(
        session.stderr().contains("bad file descriptor"),
        "stderr was {:?}",
        session.stderr()
    );
}

#[test]
fn redirection_targets_are_expanded() {
    let scratch = Scratch::new();
    let session = run_full("echo hi > $OUT\n", &[("OUT", "out.txt")], &scratch.0);
    assert_eq!(session.stdout(), "");
    assert_eq!(scratch.read("out.txt"), "hi\n");
}

// ---- expansion -------------------------------------------------------------

#[test]
fn the_exit_status_is_readable_as_a_variable() {
    // `$?` is the first thing Phase 2 makes possible that Phase 1 could not do
    // at all: the shell can finally report its own state back to the user.
    assert_eq!(run("sh -c 'exit 7'\necho $?\n").stdout(), "7\n");
    assert_eq!(run("true\necho $?\n").stdout(), "0\n");
}

#[test]
fn variables_from_the_environment_expand() {
    let session = run_with_env("echo $GREETING\n", &[("GREETING", "hello")]);
    assert_eq!(session.stdout(), "hello\n");
}

#[test]
fn expansion_happens_inside_double_quotes_but_not_single() {
    let vars = [("NAME", "world")];
    assert_eq!(
        run_with_env(r#"echo "hi $NAME""#, &vars).stdout(),
        "hi world\n"
    );
    assert_eq!(
        run_with_env("echo 'hi $NAME'", &vars).stdout(),
        "hi $NAME\n"
    );
}

#[test]
fn tilde_expands_to_home() {
    let home = std::env::var("HOME").expect("HOME must be set to run this test");
    assert_eq!(run("echo ~\n").stdout(), format!("{home}\n"));
    assert_eq!(run("echo ~/src\n").stdout(), format!("{home}/src\n"));
}

#[test]
fn an_unquoted_expansion_is_split_into_separate_arguments() {
    // `$#` is the argument count as the child sees it, which is the only way to
    // observe field splitting from outside the shell.
    let vars = [("LIST", "a b c")];
    assert_eq!(
        run_with_env("sh -c 'echo $#' sh $LIST\n", &vars).stdout(),
        "3\n"
    );
    assert_eq!(
        run_with_env(r#"sh -c 'echo $#' sh "$LIST""#, &vars).stdout(),
        "1\n"
    );
}

#[test]
fn an_unset_variable_passes_no_argument_unless_quoted() {
    assert_eq!(run("sh -c 'echo $#' sh $NOPE\n").stdout(), "0\n");
    assert_eq!(run(r#"sh -c 'echo $#' sh "$NOPE""#).stdout(), "1\n");
}

#[test]
fn the_shell_reports_its_own_pid() {
    let session = run("echo $$\n");
    let printed: u32 = session.stdout().trim().parse().expect("expected a pid");
    assert!(printed > 1, "expected a plausible pid, got {printed}");
}

#[test]
fn cd_expands_its_argument() {
    // Expansion happens before dispatch, so builtins get it for free.
    let session = run_with_env("cd $TARGET\npwd\n", &[("TARGET", "/usr")]);
    assert_eq!(session.stdout(), "/usr\n");
}

#[test]
fn unsupported_parameter_forms_are_reported_not_guessed() {
    let session = run("echo ${NAME:-fallback}\n");
    assert_eq!(session.stdout(), "");
    assert_eq!(session.code(), 2);
    assert!(
        session.stderr().contains("${NAME:-fallback}"),
        "stderr was {:?}",
        session.stderr()
    );
}

#[test]
fn command_substitution_is_refused_rather_than_ignored() {
    let session = run("echo $(date)\n");
    assert_eq!(session.stdout(), "");
    assert_eq!(session.code(), 2);
}

// ---- interactivity ---------------------------------------------------------

#[test]
fn no_prompt_when_input_is_not_a_terminal() {
    // stdin here is a pipe, so the shell is running a script and should be
    // silent about it.
    let session = run("echo hello\n");
    assert!(!session.stdout().contains("rsh>"));
    assert!(!session.stderr().contains("rsh>"));
}
