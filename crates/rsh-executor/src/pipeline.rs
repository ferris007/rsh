//! Running a pipeline as a job.
//!
//! Every external command goes through here, including a pipeline of one. That
//! is not uniformity for its own sake: with job control, a lone `sleep 30`
//! needs a process group and the terminal exactly as much as `a | b | c` does,
//! and a shell with two spawning paths has two places to get that wrong.
//!
//! ```text
//!          pipe 0              pipe 1
//!    a ──────────────► b ──────────────► c
//!    └──────────── process group 4242 ───┘
//!                        ▲
//!                        │ the terminal's foreground group,
//!                        │ if this job is in the foreground
//! ```
//!
//! # The rule pipes depend on
//!
//! A reader sees end-of-input only when *every* write end of its pipe is
//! closed, and every child inherits a copy of every pipe. No pipe descriptor is
//! closed by hand here: pipe ends are close-on-exec, `dup2` clears that flag on
//! the one or two a child was given, and the shell's own copies go when the
//! `pipes` local is dropped.

use std::io::Write;
use std::os::fd::AsRawFd;

use nix::sys::signal::Signal;
use nix::unistd::{setpgid, Pid};
use rsh_job::JobTable;
use rsh_parser::Pipeline;
use rsh_process::{Child, ExitStatus, Pipe, Redirections, Waited};

use crate::builtin::Builtin;
use crate::expand::{expand_all, Environment};
use crate::redirect::{plan_into, RedirectError};

/// Why a pipeline could not be run.
#[derive(Debug)]
pub enum PipelineError {
    /// A redirection on one of the commands could not be set up.
    Redirect(RedirectError),
    /// A pipe could not be created, or a process could not be started.
    Process(rsh_process::SpawnError),
    /// A command in the pipeline is a shell builtin.
    Builtin(String),
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Redirect(error) => write!(f, "{error}"),
            Self::Process(error) => write!(f, "{error}"),
            Self::Builtin(name) => {
                write!(f, "`{name}` is a shell builtin and cannot be a job yet")
            }
        }
    }
}

/// Everything the executor needs to place a job correctly.
pub struct Context<'a> {
    /// The shell's job table.
    pub jobs: &'a mut JobTable,
    /// Whether the shell owns a terminal and can hand it over.
    ///
    /// False for a script or a pipe. Job control is switched off entirely in
    /// that case rather than half-performed: there is no terminal to give away,
    /// nobody to type Ctrl-Z, and no reason to isolate jobs into their own
    /// groups.
    pub job_control: bool,
    /// The shell's own process group, to take the terminal back.
    pub shell_pgid: Pid,
}

/// What a prepared stage needs before anything is forked.
struct Stage {
    argv: Vec<String>,
    redirections: Redirections,
}

/// Run a pipeline, as a foreground job or a background one.
pub fn run(
    pipeline: &Pipeline,
    env: &dyn Environment,
    ctx: &mut Context<'_>,
    command_text: &str,
) -> Result<i32, PipelineError> {
    let commands = pipeline.commands();

    let pipes: Vec<Pipe> = (1..commands.len())
        .map(|_| Pipe::new())
        .collect::<Result<_, _>>()
        .map_err(PipelineError::Process)?;

    let mut stages = Vec::with_capacity(commands.len());

    for (index, command) in commands.iter().enumerate() {
        let mut redirections = Redirections::new();

        // Recorded as raw descriptors, not owned ones: the pipes outlive every
        // fork below and are closed by the shell in one place, deliberately.
        if let Some(pipe) = index.checked_sub(1).and_then(|prev| pipes.get(prev)) {
            redirections.duplicate(0, pipe.read.as_raw_fd());
        }
        if let Some(pipe) = pipes.get(index) {
            redirections.duplicate(1, pipe.write.as_raw_fd());
        }

        // Added afterwards, so an explicit redirection overrides the pipe:
        // `echo hi > f | cat` writes to the file and `cat` reads nothing.
        plan_into(command, env, &mut redirections).map_err(PipelineError::Redirect)?;

        let argv = expand_all(command.words(), env);

        if let Some(program) = argv.first() {
            if Builtin::lookup(program).is_some() {
                return Err(PipelineError::Builtin(program.clone()));
            }
        }

        stages.push(Stage { argv, redirections });
    }

    // Nothing below this line may fail in a way that leaves children unreaped.
    let _ = std::io::stdout().flush();

    let mut children = Vec::with_capacity(stages.len());
    let mut pgid: Option<Pid> = None;

    for stage in stages {
        let spawned = spawn(stage, pgid, ctx.job_control);

        if let Spawned::Running(child) = &spawned {
            // The first stage leads the group; the rest join it. Set from the
            // parent as well as the child, because either may run first after
            // the fork and the group must exist before anything can signal it.
            // One of the two calls is always redundant, and which one is not
            // knowable in advance.
            let leader = pgid.unwrap_or_else(|| child.pid());
            if ctx.job_control {
                let _ = setpgid(child.pid(), leader);
            }
            pgid.get_or_insert(leader);
        }

        children.push(spawned);
    }

    // The shell's own copies. Until these go, every reader still has a live
    // write end somewhere and would block at end-of-input.
    drop(pipes);

    let pgid = pgid.unwrap_or(ctx.shell_pgid);

    if pipeline.background() {
        return Ok(background(children, pgid, ctx, command_text));
    }

    Ok(foreground(children, pgid, ctx, command_text))
}

/// Record a background job and return to the prompt.
fn background(children: Vec<Spawned>, pgid: Pid, ctx: &mut Context<'_>, command_text: &str) -> i32 {
    let pids: Vec<Pid> = children.iter().filter_map(Spawned::pid).collect();

    if pids.is_empty() {
        return wait_all(children);
    }

    let id = ctx.jobs.add(pgid, pids.clone(), command_text.to_owned());
    println!("[{id}] {}", pids[0]);

    // A background job's status is zero: the shell did not wait, so it has
    // nothing else to report. `$?` after `cmd &` is about starting the job.
    0
}

/// Wait for a foreground job, handing it the terminal first.
fn foreground(children: Vec<Spawned>, pgid: Pid, ctx: &mut Context<'_>, command_text: &str) -> i32 {
    if ctx.job_control {
        // Giving the job the terminal is what makes it the foreground job:
        // Ctrl-C and Ctrl-Z will now reach it, and reads from the terminal will
        // succeed for it instead of stopping it with SIGTTIN.
        let _ = rsh_process::give_terminal_to(pgid);
    }

    let mut status = 0;
    let mut ended = None;
    let mut stopped = false;
    let mut live: Vec<Child> = Vec::new();

    for spawned in children {
        match spawned {
            Spawned::Failed(code) => status = code,
            Spawned::Running(child) => {
                if stopped {
                    // The whole group was suspended together, so there is
                    // nothing to wait for. Keep the child to record in the job.
                    live.push(child);
                    continue;
                }

                match await_stage(&child, ctx.job_control, command_text) {
                    Ok(Some(finished)) => {
                        status = finished.code();
                        ended = Some(finished);
                    }
                    Ok(None) => {
                        stopped = true;
                        live.push(child);
                    }
                    Err(error) => {
                        eprintln!("rsh: {error}");
                        status = rsh_process::EXIT_NOT_EXECUTABLE;
                    }
                }
            }
        }
    }

    if ctx.job_control {
        // Take the terminal back before printing anything. The shell is not the
        // foreground group at this moment, so writing to the terminal could
        // earn it a SIGTTOU — which it ignores, but the ordering is the point.
        let _ = rsh_process::give_terminal_to(ctx.shell_pgid);
    }

    if stopped {
        let pids: Vec<Pid> = live.iter().map(Child::pid).collect();
        let id = ctx.jobs.add(pgid, pids, command_text.to_owned());
        if let Some(job) = ctx.jobs.find_mut(rsh_job::JobSpec::Id(id)) {
            job.stopped();
            job.mark_reported();
        }
        // The leading newline moves past the `^Z` the terminal echoed where
        // the cursor was. Without it the notice runs into the keystroke.
        println!("\n[{id}]+  Stopped                 {command_text}");

        // 128 + SIGTSTP, the status a shell reports for a suspended command.
        return 148;
    }

    // The terminal echoed `^C` where the cursor was and the command died
    // mid-line. The shell no longer receives the signal itself — the job has
    // its own process group now — so the newline has to come from noticing how
    // the job ended rather than from a flag the handler set.
    if matches!(
        ended,
        Some(ExitStatus::Signaled(Signal::SIGINT | Signal::SIGQUIT))
    ) {
        eprintln!();
    }

    status
}

/// Wait for one stage, returning `None` if it suspended.
///
/// Without job control a stop has nowhere to go. There is no job table entry a
/// user could name, no terminal to hand back, and nothing able to resume the
/// process — so the shell continues it and says so, which is what it did before
/// job control existed. Returning `None` there would leave a stopped process
/// holding its side of the pipeline open forever.
fn await_stage(
    child: &Child,
    job_control: bool,
    command_text: &str,
) -> Result<Option<ExitStatus>, rsh_process::SpawnError> {
    loop {
        match child.wait_or_stop()? {
            Waited::Finished(status) => return Ok(Some(status)),
            Waited::Stopped(_) if job_control => return Ok(None),
            Waited::Stopped(signal) => {
                eprintln!(
                    "rsh: {command_text}: stopped by {signal}; continuing (no terminal for job control)"
                );
                child.resume()?;
            }
        }
    }
}

/// What became of one stage: a running child, or a failure to report.
enum Spawned {
    Running(Child),
    Failed(i32),
}

impl Spawned {
    fn pid(&self) -> Option<Pid> {
        match self {
            Self::Running(child) => Some(child.pid()),
            Self::Failed(_) => None,
        }
    }
}

/// Start one stage, turning a failure into a status rather than an error.
fn spawn(stage: Stage, pgid: Option<Pid>, job_control: bool) -> Spawned {
    let Some(program) = stage.argv.first() else {
        // Every word expanded away — `$UNSET | cat`. POSIX gives a command with
        // no name status zero, and its side of the pipe simply closes.
        return Spawned::Failed(0);
    };

    let path = match rsh_process::resolve(program, std::env::var_os("PATH").as_deref()) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("rsh: {error}");
            return Spawned::Failed(match error {
                rsh_process::ResolveError::NotFound { .. } => rsh_process::EXIT_NOT_FOUND,
                rsh_process::ResolveError::NotExecutable { .. } => rsh_process::EXIT_NOT_EXECUTABLE,
            });
        }
    };

    let prepared = match rsh_process::Command::new(&path, &stage.argv) {
        Ok(prepared) => {
            let prepared = prepared.redirections(stage.redirections);
            if job_control {
                prepared.process_group(pgid)
            } else {
                prepared
            }
        }
        Err(error) => {
            eprintln!("rsh: {error}");
            return Spawned::Failed(rsh_process::EXIT_NOT_EXECUTABLE);
        }
    };

    match prepared.spawn() {
        Ok(child) => Spawned::Running(child),
        Err(error) => {
            eprintln!("rsh: {program}: {error}");
            Spawned::Failed(rsh_process::EXIT_NOT_EXECUTABLE)
        }
    }
}

/// Wait for every child, and report the last one's status.
///
/// All of them are reaped, not just the last: a shell that waited only for the
/// command whose status it needed would accumulate zombies for the rest.
///
/// The status is the *last* stage's, which is POSIX and is why `grep -q x |
/// true` succeeds no matter what `grep` found.
fn wait_all(children: Vec<Spawned>) -> i32 {
    let mut status = 0;

    for child in children {
        status = match child {
            Spawned::Running(child) => match child.wait() {
                Ok(status) => status.code(),
                Err(error) => {
                    eprintln!("rsh: {error}");
                    rsh_process::EXIT_NOT_EXECUTABLE
                }
            },
            Spawned::Failed(status) => status,
        };
    }

    status
}
