//! Running a pipeline.
//!
//! `a | b | c` is three processes and two pipes, all alive at once. They are
//! not run in sequence and their output is not buffered between stages: `c`
//! starts reading before `a` has finished writing, which is why
//! `find / | head -1` answers immediately instead of after cataloguing a disk.
//!
//! ```text
//!          pipe 0              pipe 1
//!    a ──────────────► b ──────────────► c
//!    │                 │                 │
//!    └── stdout        └── stdin/stdout  └── stdin
//!        is the            are both          is the
//!        write end         pipe ends         read end
//! ```
//!
//! # The rule that makes it work
//!
//! A reader sees end-of-input when *every* write end of its pipe is closed —
//! not most of them, all. Since every child inherits a copy of every pipe, and
//! the shell holds its own copies too, a single stray descriptor anywhere makes
//! the reader wait forever.
//!
//! Two mechanisms handle that, and between them no descriptor is ever closed by
//! hand:
//!
//! * **Children**: pipe ends are close-on-exec, and `dup2` clears that flag
//!   only on the one or two a child was given. `exec` closes the rest.
//! * **The shell**: the pipes are owned by a local, and dropping it closes
//!   every end once the children are forked.

use std::io::Write;
use std::os::fd::AsRawFd;

use rsh_parser::Pipeline;
use rsh_process::{Child, Pipe, Redirections};

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
                write!(
                    f,
                    "`{name}` is a shell builtin and cannot appear in a pipeline yet"
                )
            }
        }
    }
}

/// What a prepared pipeline stage needs before anything is forked.
struct Stage {
    argv: Vec<String>,
    redirections: Redirections,
}

/// Run a pipeline, returning the status the shell should report.
///
/// Every stage is prepared first — expanded, resolved, its files opened — and
/// only then is anything forked. A pipeline that cannot be set up runs no
/// processes at all, rather than leaving half of it running and the other half
/// reporting an error.
pub fn run(pipeline: &Pipeline, env: &dyn Environment) -> Result<i32, PipelineError> {
    let commands = pipeline.commands();
    debug_assert!(
        commands.len() > 1,
        "a single command does not need a pipeline"
    );

    // One pipe between each adjacent pair. Held here, in the shell, until every
    // child has been forked — and then dropped, which closes the shell's copies
    // and lets the readers reach end-of-input.
    let pipes: Vec<Pipe> = (1..commands.len())
        .map(|_| Pipe::new())
        .collect::<Result<_, _>>()
        .map_err(PipelineError::Process)?;

    let mut stages = Vec::with_capacity(commands.len());

    for (index, command) in commands.iter().enumerate() {
        let mut redirections = Redirections::new();

        // Stdin from the previous pipe, stdout to the next. First and last
        // stages keep the shell's own, which is how a pipeline stays connected
        // to the terminal at both ends.
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
            // A builtin changes the shell's own state, so running one here
            // would mean either forking the shell — a subshell, which is
            // Phase 6 machinery — or letting `cd` in a pipeline move the real
            // shell, which no other shell does.
            if Builtin::lookup(program).is_some() {
                return Err(PipelineError::Builtin(program.clone()));
            }
        }

        stages.push(Stage { argv, redirections });
    }

    // Nothing below this line may fail in a way that leaves children unreaped.
    let _ = std::io::stdout().flush();

    let mut children = Vec::with_capacity(stages.len());
    for stage in stages {
        children.push(spawn(stage));
    }

    // The shell's own copies. Until these go, every reader still has a live
    // write end somewhere and would block at end-of-input.
    drop(pipes);

    Ok(wait_all(children))
}

/// What became of one stage: a running child, or a failure to report.
enum Spawned {
    Running(Child),
    Failed(i32),
}

/// Start one stage, turning a failure into a status rather than an error.
///
/// A pipeline is already running by the time this can fail, so there is no
/// unwinding to do: the stage reports the status a shell would have reported,
/// and its neighbours see the end-of-input or empty output that follows.
fn spawn(stage: Stage) -> Spawned {
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
        Ok(prepared) => prepared.redirections(stage.redirections),
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
/// true` succeeds no matter what `grep` found. Reporting the first failure
/// instead would be more useful and less compatible; `bash` splits the
/// difference with `pipefail`, which is a later problem.
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
