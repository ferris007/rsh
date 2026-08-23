# Process model

How `rsh` turns a line of text into a running program, and why the code is
shaped the way it is.

## `fork` + `exec`, and why they are two calls

Unix has no "run this program" call. It has one call that duplicates the
current process and another that replaces a process's image with a new
program. Everything a shell does with redirection, process groups, and signal
dispositions happens in the gap between them — in the child, after it exists
but before it becomes the target program.

```text
parent (rsh)                        child
────────────                        ─────
resolve PATH
build argv as C strings
      │
    fork() ──────────────────────►  fork() returns 0
      │                                 │
 returns child pid                  (the window: dup2, setpgid, signals)
      │                                 │
   waitpid() ◄─── SIGCHLD ───┐      execvp()  ── image replaced ──►  /bin/echo
      │                      │          │
 reap, decode status         └──────  exit / killed by signal
```

That window is the entire reason shells are interesting. It is also the most
dangerous place in the program.

## The window is not ordinary Rust

After `fork()` the child has a *copy* of the parent's address space, but only
one thread: the one that called `fork`. Any mutex another thread held at that
instant is now locked forever, because the thread that would have unlocked it
does not exist in the child. The allocator has such a mutex. So does the
runtime's stdout lock.

POSIX resolves this by permitting only [async-signal-safe] functions between
`fork` and `exec`. `malloc` is not one of them. Which means, in Rust: **no
allocation, no formatting, no `println!`, no anything that might allocate.**

A child that violates this does not fail loudly. It deadlocks, occasionally,
under load, in a process that no longer has a terminal to complain to.

[async-signal-safe]: https://man7.org/linux/man-pages/man7/signal-safety.7.html

## How `rsh` stays out of trouble

Everything fallible or allocating happens **before** the fork:

1. Resolve the program name against `PATH` in the parent.
2. Build `argv` and `envp` as `CString`s in the parent.
3. `fork`.
4. In the child: only `execvp` with the already-built pointers, and `_exit` if
   it fails.

The child allocates nothing. It also uses `_exit`, not `exit` — the latter
runs `atexit` handlers and flushes the parent's inherited stdio buffers, which
would duplicate output the parent has already queued but not yet written.

This is why `rsh-process` is a separate crate: the invariant is auditable
because the code that must uphold it is small and lives in one place. Later
phases (redirection, process groups) add work to the window, and every
addition has to answer the same question — *is this async-signal-safe?*

## Exit status

`waitpid` yields an encoded `int`, not a number the user typed. `rsh` decodes
it into a real type rather than passing the raw value around:

| Outcome                | Status                | What the shell reports |
| ---------------------- | --------------------- | ---------------------- |
| exited normally        | `Exited(code)`        | `code`                 |
| killed by a signal     | `Signaled(sig)`       | `128 + sig`            |
| stopped (Phase 6)      | `Stopped(sig)`        | job suspended          |

The `128 + signal` convention is what Bash and POSIX shells report, so
`command || echo $?` behaves the way scripts expect. Two failures are reserved
by convention and `rsh` honours them:

- `127` — command not found
- `126` — found, but not executable

These are not arbitrary: tools in the wild branch on them.

## `PATH` lookup

`execvp` would search `PATH` itself, but `rsh` resolves the path in the parent
instead. Doing it early means the "command not found" case is an ordinary
`Result` in normal Rust — reported with a message, `127`, and no child process
at all — rather than an error discovered inside the window, where the only
available reporting mechanism is an exit code.

Lookup follows the POSIX rules: a name containing `/` is used as-is and never
searched for; otherwise each `PATH` entry is tried in order and the first
entry that is a regular file with execute permission wins. An empty entry
means the current directory, which is a genuine POSIX rule and a genuine
security footgun — `rsh` implements it and says so here rather than silently
diverging.

## What is deliberately missing

Phase 1 spawns one process, waits for it, and moves on. There are no process
groups yet, so `Ctrl-C` reaches the child only because it shares the shell's
group by inheritance — which is also why it reaches the shell. Fixing that
properly requires `setpgid` and `tcsetpgrp`, and that is Phase 5 and Phase 6
work, not something to bolt on early.
