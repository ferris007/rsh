//! Commands the shell must run itself.
//!
//! A builtin is not "a command that happens to be fast". It is a command that
//! *cannot* be a child process, because its whole effect is a change to the
//! shell's own state. `/usr/bin/cd` could exist, and would do nothing useful:
//! it would change the working directory of a process that exits a moment
//! later, leaving the shell exactly where it was.
//!
//! That is the test for what belongs here. Phase 6 adds `jobs`, `fg`, and `bg`
//! for the same reason — the job table lives in this process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use nix::sys::signal::Signal;
use rsh_job::{JobSpec, JobState, JobTable};

use crate::shell::Outcome;

/// Status returned by a builtin that was used incorrectly.
const EXIT_USAGE: i32 = 1;

/// Status for `exit` with a non-numeric argument, matching POSIX shells.
const EXIT_BAD_ARGUMENT: i32 = 2;

/// Everything a builtin may reach outside its own arguments.
///
/// Passed in rather than reached for, so the set of things a builtin can touch
/// is visible in one place. `jobs`, `fg`, and `bg` exist *because* they need
/// this — they are builtins for the same reason `cd` is, in that their whole
/// effect is on state that only the shell process has.
pub(crate) struct Context<'a> {
    pub jobs: &'a mut JobTable,
    pub job_control: bool,
    pub shell_pgid: nix::unistd::Pid,
    pub shell_modes: Option<&'a rsh_terminal::Modes>,
    pub last_status: i32,
}

/// A command implemented by the shell itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Builtin {
    Cd,
    Exit,
    Jobs,
    Fg,
    Bg,
}

impl Builtin {
    /// Match a command name against the builtin table.
    ///
    /// Builtins are checked before `PATH`, which is why `cd` works even on a
    /// system that also ships a `/usr/bin/cd`.
    pub(crate) fn lookup(name: &str) -> Option<Self> {
        match name {
            "cd" => Some(Self::Cd),
            "exit" => Some(Self::Exit),
            "jobs" => Some(Self::Jobs),
            "fg" => Some(Self::Fg),
            "bg" => Some(Self::Bg),
            _ => None,
        }
    }

    /// Run the builtin, returning whether to continue and the new `$?`.
    ///
    /// `args` excludes the builtin's own name and has already been expanded, so
    /// `cd $HOME` and `cd /home/ferris` are indistinguishable here — which is
    /// the point of doing expansion before dispatch.
    pub(crate) fn run(self, args: &[String], ctx: &mut Context<'_>) -> (Outcome, i32) {
        match self {
            Self::Cd => (Outcome::Continue, cd(args)),
            Self::Exit => exit(args, ctx.last_status),
            Self::Jobs => (Outcome::Continue, jobs(ctx)),
            Self::Fg => (Outcome::Continue, fg(args, ctx)),
            Self::Bg => (Outcome::Continue, bg(args, ctx)),
        }
    }
}

/// Change the working directory.
///
/// Handles the three forms a POSIX shell provides: no argument (go home), `-`
/// (go back), and an explicit path.
fn cd(args: &[String]) -> i32 {
    if args.len() > 1 {
        eprintln!("rsh: cd: too many arguments");
        return EXIT_USAGE;
    }

    // `cd -` prints where it landed. That is not decoration: the argument is
    // `$OLDPWD`, which the user cannot see, so without the echo they would not
    // know where they now are.
    let mut announce = false;

    let target: PathBuf = match args.first().map(String::as_str) {
        None | Some("~") => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home),
            None => {
                eprintln!("rsh: cd: HOME not set");
                return EXIT_USAGE;
            }
        },
        Some("-") => match std::env::var_os("OLDPWD") {
            Some(old) => {
                announce = true;
                PathBuf::from(old)
            }
            None => {
                eprintln!("rsh: cd: OLDPWD not set");
                return EXIT_USAGE;
            }
        },
        Some(path) => PathBuf::from(path),
    };

    // Read the old directory *before* moving, so OLDPWD is accurate even if
    // the change fails halfway through a symlinked path.
    let previous = std::env::current_dir().ok();

    if let Err(error) = std::env::set_current_dir(&target) {
        eprintln!("rsh: cd: {}: {error}", target.display());
        return EXIT_USAGE;
    }

    update_pwd(previous.as_deref());

    if announce {
        if let Ok(current) = std::env::current_dir() {
            println!("{}", current.display());
        }
    }

    0
}

/// Keep `PWD` and `OLDPWD` in step with the process's real directory.
///
/// These are environment variables, not shell state, because child processes
/// read them. A program that prints `$PWD` and a program that calls `getcwd()`
/// should not disagree, so `PWD` is set from `current_dir()` — the kernel's
/// answer — rather than from the path the user typed, which may be relative or
/// contain `..`.
fn update_pwd(previous: Option<&Path>) {
    if let Some(previous) = previous {
        std::env::set_var("OLDPWD", OsString::from(previous));
    }
    if let Ok(current) = std::env::current_dir() {
        std::env::set_var("PWD", OsString::from(current));
    }
}

/// Leave the shell.
///
/// With no argument, exits with the status of the last command — so
/// `some-command; exit` propagates that command's result, which is what makes
/// `exit` usable as the last line of a script.
fn exit(args: &[String], last_status: i32) -> (Outcome, i32) {
    match args {
        [] => (Outcome::Exit(last_status), last_status),
        [code] => match code.parse::<i32>() {
            Ok(code) => (Outcome::Exit(code), code),
            Err(_) => {
                // POSIX shells exit anyway, with status 2. Refusing to exit
                // would be worse: `exit` is how a user gets out, and a typo
                // should not trap them in the shell.
                eprintln!("rsh: exit: {code}: numeric argument required");
                (Outcome::Exit(EXIT_BAD_ARGUMENT), EXIT_BAD_ARGUMENT)
            }
        },
        _ => {
            // Here the shell does *not* exit: with several arguments it cannot
            // tell which status was meant, and guessing would be worse than
            // asking again.
            eprintln!("rsh: exit: too many arguments");
            (Outcome::Continue, EXIT_USAGE)
        }
    }
}

/// List the jobs the shell is tracking.
///
/// Deliberately does not reap: `jobs` reports what the shell knows, and the
/// knowing is refreshed once per prompt. A `jobs` that went and collected
/// statuses would show a different answer from the notification printed a
/// moment earlier, for no reason a user could see.
fn jobs(ctx: &mut Context<'_>) -> i32 {
    for job in ctx.jobs.iter() {
        println!(
            "[{}]{}  {:<22}  {}",
            job.id(),
            ctx.jobs.marker(job.id()),
            job.state().describe(),
            job.command()
        );
    }
    0
}

/// Bring a job to the foreground.
///
/// Three steps, in this order: hand over the terminal, continue the job, wait
/// for it. Continuing before the handover would let the job read from a
/// terminal it does not own and be stopped again with `SIGTTIN` — the resume
/// would appear to do nothing.
fn fg(args: &[String], ctx: &mut Context<'_>) -> i32 {
    if !ctx.job_control {
        eprintln!("rsh: fg: no job control without a terminal");
        return EXIT_USAGE;
    }

    let Some((id, pgid, command)) = select(args, ctx, "fg") else {
        return EXIT_USAGE;
    };

    println!("{command}");

    // Put the job's terminal modes back before handing over. A job suspended
    // inside `vim` needs raw mode again; giving it a terminal in the shell's
    // canonical mode would bring it back visibly broken.
    if let Some(job) = ctx.jobs.find(JobSpec::Id(id)) {
        if let Some(modes) = job.modes() {
            let _ = rsh_terminal::restore(modes);
        }
    }

    let _ = rsh_terminal::give_to(pgid);
    if let Err(error) = nix::sys::signal::killpg(pgid, Signal::SIGCONT) {
        eprintln!("rsh: fg: {error}");
        let _ = rsh_terminal::give_to(ctx.shell_pgid);
        return EXIT_USAGE;
    }

    if let Some(job) = ctx.jobs.find_mut(JobSpec::Id(id)) {
        job.resumed();
        job.mark_reported();
    }
    ctx.jobs.make_current(id);

    let status = wait_for(ctx, id, &command);

    // Same as any other foreground job: snapshot what the job left behind if it
    // suspended again, then put the shell's own settings back.
    let resumed_modes = rsh_terminal::snapshot();
    let _ = rsh_terminal::give_to(ctx.shell_pgid);
    if let Some(modes) = ctx.shell_modes {
        let _ = rsh_terminal::restore(modes);
    }
    if let Some(job) = ctx.jobs.find_mut(JobSpec::Id(id)) {
        if job.state() == JobState::Stopped {
            job.remember_modes(resumed_modes);
        }
    }

    status
}

/// Resume a job in the background.
///
/// The same `SIGCONT`, without the terminal and without the wait. Which is the
/// entire difference between `fg` and `bg`.
fn bg(args: &[String], ctx: &mut Context<'_>) -> i32 {
    if !ctx.job_control {
        eprintln!("rsh: bg: no job control without a terminal");
        return EXIT_USAGE;
    }

    let Some((id, pgid, command)) = select(args, ctx, "bg") else {
        return EXIT_USAGE;
    };

    if let Err(error) = nix::sys::signal::killpg(pgid, Signal::SIGCONT) {
        eprintln!("rsh: bg: {error}");
        return EXIT_USAGE;
    }

    if let Some(job) = ctx.jobs.find_mut(JobSpec::Id(id)) {
        job.resumed();
        job.mark_reported();
    }

    println!("[{id}]+ {command} &");
    0
}

/// Resolve a job specifier to something the caller can act on.
fn select(
    args: &[String],
    ctx: &Context<'_>,
    builtin: &str,
) -> Option<(rsh_job::JobId, nix::unistd::Pid, String)> {
    if args.len() > 1 {
        eprintln!("rsh: {builtin}: too many arguments");
        return None;
    }

    let spec = match JobSpec::parse(args.first().map(String::as_str)) {
        Ok(spec) => spec,
        Err(error) => {
            eprintln!("rsh: {builtin}: {error}");
            return None;
        }
    };

    match ctx.jobs.find(spec) {
        Some(job) if job.state().is_alive() => {
            Some((job.id(), job.pgid(), job.command().to_owned()))
        }
        Some(job) => {
            eprintln!("rsh: {builtin}: job {} has finished", job.id());
            None
        }
        None => {
            eprintln!("rsh: {builtin}: no such job");
            None
        }
    }
}

/// Wait for a resumed job, which may finish or stop again.
fn wait_for(ctx: &mut Context<'_>, id: rsh_job::JobId, command: &str) -> i32 {
    loop {
        let events = rsh_process::collect_child_events_blocking();
        ctx.jobs.apply(&events);

        let Some(job) = ctx.jobs.find(JobSpec::Id(id)) else {
            return 0;
        };

        match job.state() {
            JobState::Done(status) => {
                // Marked reported so the next prompt does not announce a job
                // the user just watched finish in the foreground.
                let code = status.code();
                if let Some(job) = ctx.jobs.find_mut(JobSpec::Id(id)) {
                    job.mark_reported();
                }
                return code;
            }
            JobState::Stopped => {
                println!("\n[{id}]+  Stopped                 {command}");
                if let Some(job) = ctx.jobs.find_mut(JobSpec::Id(id)) {
                    job.mark_reported();
                }
                return 148;
            }
            JobState::Running => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_recognised_by_exact_name() {
        assert_eq!(Builtin::lookup("cd"), Some(Builtin::Cd));
        assert_eq!(Builtin::lookup("exit"), Some(Builtin::Exit));
        assert_eq!(Builtin::lookup("echo"), None);
        assert_eq!(Builtin::lookup("CD"), None);
        assert_eq!(Builtin::lookup(""), None);
    }

    #[test]
    fn exit_without_an_argument_carries_the_last_status() {
        assert_eq!(exit(&[], 7), (Outcome::Exit(7), 7));
        assert_eq!(exit(&[], 0), (Outcome::Exit(0), 0));
    }

    #[test]
    fn exit_takes_an_explicit_status() {
        assert_eq!(exit(&["3".to_owned()], 0), (Outcome::Exit(3), 3));
    }

    #[test]
    fn exit_with_a_non_numeric_argument_still_exits() {
        assert_eq!(exit(&["banana".to_owned()], 0), (Outcome::Exit(2), 2));
    }

    #[test]
    fn exit_with_several_arguments_does_not_exit() {
        let args = ["1".to_owned(), "2".to_owned()];
        assert_eq!(exit(&args, 0), (Outcome::Continue, EXIT_USAGE));
    }

    // `cd` is exercised end-to-end in the integration tests instead: it mutates
    // process-wide state (the working directory and the environment), which
    // makes it unsafe to assert against from a multi-threaded test harness
    // where another test may be reading that same state.
}
