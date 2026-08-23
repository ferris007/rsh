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

- [ ] Interactive prompt
- [ ] Read input
- [ ] Basic command parsing
- [ ] Command lookup through `PATH`
- [ ] Process creation
- [ ] Process execution
- [ ] Exit status handling
- [ ] Built-in `cd`
- [ ] Built-in `exit`

**Systems concepts:** processes, `fork`, `exec`, `wait`, environment variables.

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

- [ ] Lexer
- [ ] Tokens
- [ ] Quoting
- [ ] Escaping
- [ ] Environment expansion
- [ ] Command AST
- [ ] Pipeline AST
- [ ] Redirection AST
- [ ] Parser error reporting

**Goal:** separate parsing from execution.

---

## Phase 3 — I/O redirection

- [ ] stdout redirection
- [ ] stdin redirection
- [ ] stderr redirection
- [ ] append mode
- [ ] file descriptor duplication
- [ ] descriptor inheritance

**Systems concepts:** file descriptors, `dup2`, inheritance, Unix I/O.

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

```text
experiments/
├── fork_exec/
├── process_groups/
├── signals/
├── pipes/
├── epoll/
├── pty/
├── file_descriptors/
└── namespaces/
```
