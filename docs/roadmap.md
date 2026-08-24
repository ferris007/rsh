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

- [ ] Anonymous pipes
- [ ] Multiple commands
- [ ] Pipeline process management
- [ ] Pipeline exit status
- [ ] Broken-pipe handling
- [ ] Backpressure experiments

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

- [ ] `SIGINT`
- [ ] `SIGTERM`
- [ ] `SIGCHLD`
- [ ] `SIGTSTP`
- [ ] `SIGCONT`
- [ ] Child-process reaping
- [ ] Signal-safe communication with the shell
- [ ] Graceful shutdown

---

## Phase 6 — Job control

- [ ] Background processes
- [ ] Process groups
- [ ] Foreground process group
- [ ] Job table
- [ ] `jobs`
- [ ] `fg`
- [ ] `bg`
- [ ] Suspend/resume
- [ ] Child state tracking

**Systems concepts:** sessions, process groups, controlling terminals.

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

- [ ] Raw terminal mode
- [ ] Terminal state management
- [ ] Terminal resize handling
- [ ] TTY detection
- [ ] PTY experiments
- [ ] Interactive child processes
- [ ] Terminal restoration after crashes

---

## Phase 8 — Interactive experience

Only after the OS fundamentals work, and kept separate from the
process-management core.

- [ ] Command history
- [ ] History persistence
- [ ] Arrow-key navigation
- [ ] Tab completion
- [ ] Command suggestions
- [ ] Environment completion
- [ ] Better error messages
- [ ] Config file

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

- [ ] `epoll` on Linux
- [ ] `kqueue` on BSD/macOS
- [ ] event notification
- [ ] non-blocking I/O
- [ ] child-process events
- [ ] event-driven architecture

Don't reach for Tokio here. Understand what it abstracts first.

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

- [ ] Criterion benchmarks
- [ ] tracing
- [ ] structured diagnostics
- [ ] flamegraphs
- [ ] allocation profiling
- [ ] syscall analysis
- [ ] performance regression tests

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
