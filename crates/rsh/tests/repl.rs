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

use std::io::Write;
use std::process::{Command, Output, Stdio};

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
    let mut command = Command::new(RSH);
    for (name, value) in vars {
        command.env(name, value);
    }

    let mut child = command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
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
    let session = run("echo hi | grep hi\n");
    let stderr = session.stderr();
    assert!(stderr.contains("pipelines"), "stderr was {stderr:?}");
    assert!(stderr.contains("phase 4"), "stderr was {stderr:?}");
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
    let session = run("echo hi > out\n");
    let stderr = session.stderr();
    assert!(stderr.contains('^'), "stderr was {stderr:?}");
    assert!(stderr.contains("phase 3"), "stderr was {stderr:?}");
    // The refusal has to happen before anything runs: `>` must not have
    // created the file on the way to being declined.
    assert!(!std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("out")
        .exists());
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
    let session = run("echo hi | grep hi\necho still here\n");
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
