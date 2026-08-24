# Job control

A **job** is one pipeline. Job control is the ability to have several of them
at once, move them between the foreground and the background, and suspend and
resume them — all through a single terminal that only one of them can use at a
time.

Almost everything in it follows from one piece of kernel state.

## The foreground process group

A terminal has exactly one foreground process group, set with `tcsetpgrp`. That
single variable decides three things:

- which processes receive `SIGINT` when Ctrl-C is typed, and `SIGTSTP` for
  Ctrl-Z;
- which processes may **read** from the terminal — anyone else gets `SIGTTIN`
  and is stopped;
- which may write, if the terminal's `TOSTOP` flag is set.

Running a job in the foreground *is* giving it that slot. Suspending it is
taking the slot back. Once that clicks, the rest of job control is bookkeeping.

Demonstrated in
[`experiments/process_groups`](../experiments/process_groups/), which is also
the explanation for why `cat &` stops the instant it starts.

## Every job gets a group

```text
   rsh                          process group 4200
    │
    ├── [1]  cat f | grep x     process group 4242   ← foreground
    │         │        │
    │         └────────┴─ both stages, one group, one Ctrl-C
    │
    └── [2]  sleep 300          process group 4251   ← background, untouched
```

One `killpg` reaches every stage of a pipeline, including any children the
stages started themselves. The shell never needs to know how many processes a
job turned into.

`setpgid` is called in **both** the parent and the child, deliberately. Either
may be scheduled first after `fork`, and the group has to exist before anything
can signal it or hand it the terminal. So both do it, one of the two calls is
always redundant, and which one is redundant is not knowable in advance.

## What `fg` actually is

```text
fg:   tcsetpgrp(job)  →  SIGCONT  →  wait  →  tcsetpgrp(shell)
bg:                      SIGCONT
```

That is the whole difference between them: `bg` skips the terminal and the wait.

The order in `fg` matters. Continuing the job *before* handing over the terminal
would let it read a terminal it does not own yet, earning it `SIGTTIN` and
stopping it again — the resume would appear to do nothing at all.

## The shell has to protect itself, then undo that for children

`tcsetpgrp` is a terminal operation, so a shell taking the terminal back from a
job it just suspended is a non-foreground process touching the terminal. The
default action for `SIGTTOU` is to stop the process — so a shell that did
nothing would freeze itself at the exact moment the user pressed Ctrl-Z, leaving
a stopped terminal and no shell able to unstop it.

`rsh` therefore ignores `SIGTSTP`, `SIGTTIN`, and `SIGTTOU`.

And then resets all three to their defaults in every child, because **`SIG_IGN`
is inherited across `exec`**. Without the reset, the shell's self-protection
would silently become a property of every program it runs, and Ctrl-Z would stop
nothing. This is the third time the same trap has come up in this project —
`SIGPIPE` in Phase 4, `SIGINT` in Phase 5 — and it is the reason the shell uses
*handlers* for `SIGINT` and `SIGQUIT`: `exec` resets those for free.

| | survives `exec`? |
| --- | --- |
| a handler | no — reset to the default action |
| `SIG_IGN` | **yes** |

## `SIGCHLD`, finally

Phase 5 deliberately left `SIGCHLD` out: a shell that waits for every child
synchronously already knows when they finish. Background jobs are what change
that. A job can now outlive the command that started it, and the shell needs to
learn about a death it was not waiting for.

The handler does not reap. It sets a flag, and the collecting happens at the top
of the loop with `waitpid(-1, WNOHANG | WUNTRACED | WCONTINUED)`.

Reaping *in* the handler would race with whatever `waitpid` the shell is already
blocked in for a foreground job: the handler would collect a status the main
loop is waiting for, and the main loop would then wait forever for a child that
no longer exists.

`WUNTRACED` and `WCONTINUED` widen the question from "did it die" to "did it
change state", which is the difference between a shell that can only notice
completed jobs and one that can track suspended ones.

## Notifications come at the prompt

A finished background job is announced at the *next* prompt, not the instant it
happens. A notification arriving mid-keystroke would write over whatever the
user was typing. Every shell does this, and the top of the read loop is the
quiet moment.

Each state change is reported once. Resuming a job makes it reportable again, so
`bg` followed by the job finishing produces two lines rather than one or three.

## Job numbers are for typing

`[1]`, `[2]`, and the `+`/`-` markers for the current and previous job. Numbers
are **reused** once a job is gone: counting up forever would have a user typing
`%143` on a shell that has been open all day.

`fg` with no argument means the current job, which is the one most recently
started, resumed, or suspended. Getting that default right matters more than
supporting `%1` at all — it is what people actually type.

## Leaving with jobs

Only *stopped* jobs earn a warning:

```console
rsh> exit
rsh: there are stopped jobs
rsh> exit
$
```

A running background job carries on perfectly well without a shell. A stopped
one would be left suspended with nothing able to resume it — a process leaked in
a state the user cannot see. The shell states the consequence once and lets the
second attempt through; the decision stays with the user.

## No terminal, no job control

A shell reading a script from a pipe has no terminal to hand over, nobody to
type Ctrl-Z, and no reason to isolate jobs into groups. `rsh` switches job
control off entirely in that case rather than half-performing it: children run
in the shell's own process group, exactly as they did before this phase.

One consequence worth knowing: a process that stops itself under a
non-interactive shell is *continued* rather than suspended, because there is
nowhere to put a job nobody can resume. That was Phase 5's behaviour and it
remains the right answer whenever job control is off.

## Not implemented

- **Builtins as jobs.** `cd | cat` and `jobs &` are refused. Running a builtin
  in a job means forking a subshell, which is a larger change than it sounds:
  the child would run shell code rather than `exec` immediately.
- **`disown`, `wait`, `kill %1`.** The job table supports them; the builtins are
  not written.
- **`SIGHUP` to jobs on exit.** The shell leaves running jobs alone rather than
  hanging them up.
- **Terminal state per job.** A job suspended in the middle of changing terminal
  modes leaves them changed. That is Phase 7.
