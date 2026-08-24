# Roadmap

The roadmap is intentionally long-lived. The repository does not become
valuable at the end of it — the architecture notes, experiments, benchmarks,
and decisions recorded along the way are part of the project.

Each phase builds only on primitives established by earlier phases.

---

## Phase 0 — Project foundation

- [x] Rust workspace and project structure
- [x] CI
- [x] Formatting and linting
- [x] Unit/integration test framework
- [x] Linux development environment
- [x] Basic documentation
- [x] Architecture overview

**Goal:** establish a clean systems-project foundation.

---

## Phase 1 — Interactive shell

```text
$ rsh
rsh> echo hello
hello
rsh> pwd
/home/user
rsh>
```

- [x] Interactive prompt
- [x] Read input
- [x] Basic command parsing
- [x] Command lookup through `PATH`
- [x] Process creation
- [x] Process execution
- [x] Exit status handling
- [x] Built-in `cd`
- [x] Built-in `exit`

**Systems concepts:** processes, `fork`, `exec`, `wait`, environment variables.

Notes: [`docs/process-model.md`](process-model.md). Words are not expanded yet
(`$HOME` is literal until Phase 2), and operators are recognised and refused
rather than silently treated as arguments.

---

## Phase 2 — Parser

Move away from naïve string splitting. `echo hello | grep hello > result.txt`
becomes structure:

```text
Pipeline
 ├── Command
 │    ├── echo
 │    └── hello
 │
 └── Command
      ├── grep
      └── hello
           │
           └── stdout → result.txt
```

- [x] Lexer
- [x] Tokens
- [x] Quoting
- [x] Escaping
- [x] Environment expansion
- [x] Command AST
- [x] Pipeline AST
- [x] Redirection AST
- [x] Parser error reporting

**Goal:** separate parsing from execution.

Notes: [`docs/parsing.md`](parsing.md). The tree is complete before anything can
run it — the shell parses `echo hi | grep hi > out` fully and then declines to
execute it, naming the phase that will. Expansion is parsed here but *evaluated*
in the executor, because resolving `$HOME` means reading the environment and
this crate is not allowed to.

---

## Phase 3 — I/O redirection

- [x] stdout redirection
- [x] stdin redirection
- [x] stderr redirection
- [x] append mode
- [x] file descriptor duplication
- [x] descriptor inheritance

**Systems concepts:** file descriptors, `dup2`, inheritance, Unix I/O.

Notes: [`docs/redirection.md`](redirection.md), and
[`experiments/file_descriptors`](../experiments/file_descriptors/), which exists
because `dup2(fd, fd)` silently does nothing — including not clearing
close-on-exec — and that cost this phase a real bug.

Not implemented: here-documents (`<<`, needs multi-line input), `>&-` to close a
descriptor, and `>|` for noclobber.

---

## Phase 4 — Pipelines

```text
             pipe
Command A ─────────► Command B
                       │
                     pipe
                       ▼
                    Command C
```

- [x] Anonymous pipes
- [x] Multiple commands
- [x] Pipeline process management
- [x] Pipeline exit status
- [x] Broken-pipe handling
- [x] Backpressure experiments

Notes: [`docs/pipelines.md`](pipelines.md), and
[`experiments/pipes`](../experiments/pipes/), which exists because Rust ignores
SIGPIPE before `main` and `SIG_IGN` survives `exec` — so every child of a
Rust-written shell inherits it, and `yes | head` quietly loses the mechanism
that stops the producer.

Not implemented: builtins in a pipeline (needs a subshell, Phase 6),
`set -o pipefail`, and `PIPESTATUS`.

---

## Phase 5 — Signals

```text
Ctrl-C
  ↓
terminal
  ↓
foreground process group
  ↓
SIGINT
```

- [x] `SIGINT`
- [x] `SIGTERM`
- [x] `SIGCHLD` — delivered in Phase 6, where it has something to report
- [x] `SIGTSTP`
- [x] `SIGCONT`
- [x] Child-process reaping
- [x] Signal-safe communication with the shell
- [x] Graceful shutdown

Notes: [`docs/signals.md`](signals.md), and
[`experiments/signals`](../experiments/signals/), which answers the question
this phase is really about — a group-directed signal goes to a group, and that
one fact shapes everything job control does later.

`SIGTSTP` is *handled*, not honoured: the shell notices the stop and continues
the child, because there is nowhere to put a suspended job until there is a job
table. Before this phase, `waitpid` never returned for a stopped child and
Ctrl-Z wedged the shell permanently.

`SIGCHLD` is deliberately not implemented. The shell waits for its children
synchronously, so an asynchronous death notification has nothing to tell it. It
earns its place in Phase 6, when a job can outlive the command that started it.
Installing a handler now would be machinery with no purpose.

---

## Phase 6 — Job control

- [x] Background processes
- [x] Process groups
- [x] Foreground process group
- [x] Job table
- [x] `jobs`
- [x] `fg`
- [x] `bg`
- [x] Suspend/resume
- [x] Child state tracking

**Systems concepts:** sessions, process groups, controlling terminals.

Notes: [`docs/job-control.md`](job-control.md), and
[`experiments/process_groups`](../experiments/process_groups/), which explains
why `cat &` stops the instant it starts.

This phase also delivers the `SIGCHLD` item deferred from Phase 5. It had
nothing to report while every child was waited for synchronously; background
jobs are what changed that.

Not implemented: builtins as jobs (needs a subshell), `disown`, `wait`,
`kill %1`, and `SIGHUP` to jobs on exit.

---

## Phase 7 — Terminal management

```text
rsh
 │
 ├── TTY
 │
 └── foreground process group
        │
        └── interactive application (vim, top, ssh)
```

- [x] Raw terminal mode
- [x] Terminal state management
- [x] Terminal resize handling
- [x] TTY detection
- [x] PTY experiments
- [x] Interactive child processes
- [x] Terminal restoration after crashes

Notes: [`docs/terminal.md`](terminal.md), and [`experiments/pty`](../experiments/pty/),
which answers why a program's output stops appearing the moment you pipe it.

Raw mode is provided and tested but not yet used by the REPL — line editing is
Phase 8. An untested capability adopted a phase later is a capability debugged a
phase later.

Not implemented: `terminfo`, the alternate screen, and allocating a pty per job
(which is what `script` and `tmux` do, and is a different program).

---

## Phase 8 — Interactive experience

Only after the OS fundamentals work, and kept separate from the
process-management core.

- [x] Command history
- [x] History persistence
- [x] Arrow-key navigation
- [x] Tab completion
- [x] Command suggestions
- [x] Environment completion
- [x] Better error messages
- [x] Config file

Notes: [`docs/line-editing.md`](line-editing.md).

Line editing means *giving up* what the terminal driver already does — backspace,
Ctrl-U, Ctrl-C and the rest worked from Phase 1 without a line of code, and raw
mode discards all of it. The shell is not adding history to an existing editor;
it is replacing the terminal's editor with its own.

Not implemented: reverse incremental search, a kill ring, multi-line editing
(the parser has no continuation prompt either), syntax highlighting, and
programmable completion. Lines longer than the terminal is wide are not redrawn
correctly, which needs character-width handling this phase deliberately stops
short of.

---

## Phase 9 — Event-driven architecture

```text
             ┌─────────────┐
             │ Event Loop  │
             └──────┬──────┘
                    │
        ┌───────────┼───────────┐
        ▼           ▼           ▼
      stdin       signals     children
        │           │           │
        └───────────┴───────────┘
                    │
                 scheduler
```

- [x] `epoll` on Linux
- [x] `kqueue` on BSD/macOS
- [x] event notification
- [x] non-blocking I/O — see the note
- [ ] child-process events — see the note
- [x] event-driven architecture

Don't reach for Tokio here. Understand what it abstracts first.

Notes: [`docs/events.md`](events.md), and [`experiments/epoll`](../experiments/epoll/),
which reproduces the race that makes a signal flag insufficient — 800ms slept
through with a flag alone, 0ms with a self-pipe.

`rsh-event` is about two hundred lines and is a very small `mio`, which is the
layer Tokio sits on. An async runtime is a scheduler above this line; below it
is a poller saying which descriptors are ready.

**Non-blocking I/O** is used only where the shell owns the descriptor outright.
`O_NONBLOCK` belongs to the open file description, so setting it on standard
input sets it for every child too — a `cat` that gets `EAGAIN` from a terminal
is a bug the shell caused. Readiness plus a blocking read is equivalent and
safe.

**Child-process events** are left unchecked deliberately. `SIGCHLD` still
arrives as a signal rather than as a descriptor; Linux's `pidfd` would fix that
and has no portable equivalent. The executor also still blocks in `waitpid`,
which is the larger remaining piece.

---

## Phase 10 — Performance & observability

```text
rsh benchmark
────────────────────────
startup       1.82 ms
echo          2.14 ms
pipeline      4.91 ms
memory        3.7 MB
```

- [x] Criterion benchmarks
- [x] tracing — see the note
- [x] structured diagnostics
- [ ] flamegraphs — a recipe, not a result
- [x] allocation profiling
- [ ] syscall analysis — a recipe, not a result
- [x] performance regression tests

Notes: [`docs/performance.md`](performance.md).

The headline: starting a process costs 492µs, and parsing the command line that
describes it costs 1.3µs. A shell that optimised its parser would be polishing a
rounding error.

The benchmarks found something nobody had timed — "did you mean", added in Phase
8, costs 61ms, because it reads every directory on `PATH`. Measured, traced to
WSL's Windows filesystem bridge rather than to the algorithm, and **left alone**.
That is what measuring before optimising looks like when the answer is "don't".

**tracing** is `rsh-trace`, not the crate. A shell's most interesting moment is
the window between `fork` and `exec`, where nothing may allocate, and a
subscriber that formats an event into a `String` is exactly what must not happen
there.

**Regression tests** count allocations rather than time. CI wall-clock varies
several-fold, so a threshold loose enough to pass is too loose to catch
anything.

**Flamegraphs and syscall analysis** are documented procedures. This machine has
no `perf`, `strace`, or `valgrind`, and printing plausible output without having
produced any would be worse than leaving the boxes unchecked.

---

## Phase 11 — Linux systems experiments

Each experiment answers one concrete systems question with a runnable program
and a recorded observation.

See [`experiments/README.md`](../experiments/README.md) for the format and the
index of what exists so far.

```text
experiments/
├── fork_exec/          done — why the child must _exit, not exit
├── process_groups/
├── signals/
├── pipes/
├── epoll/
├── pty/
├── file_descriptors/
└── namespaces/
```
