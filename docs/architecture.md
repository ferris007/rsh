# Architecture

## The shape of the problem

A shell is a small program with an unusually large surface against the
operating system. Almost none of its difficulty is in the language it parses;
nearly all of it is in the process, descriptor, signal, and terminal state it
has to manage correctly while a user is watching.

`whelk` is therefore structured around that state, not around the grammar.

## Layers

```text
                 ┌──────────────────────────┐
   user input →  │  whelk (binary)            │  REPL, line editing, prompt
                 └────────────┬─────────────┘
                              │  source text
                 ┌────────────▼─────────────┐
                 │  whelk-parser              │  lexer → AST
                 └────────────┬─────────────┘
                              │  AST
                 ┌────────────▼─────────────┐
                 │  whelk-executor            │  expansion, builtins, redirection,
                 └────────────┬─────────────┘   pipeline assembly, shell state
                              │  "run this program with these fds"
                 ┌────────────▼─────────────┐
                 │  whelk-process             │  fork / exec / wait, PATH lookup,
                 └────────────┬─────────────┘  pipes, signals, process groups
                              │
                 ┌────────────▼─────────────┐
                 │  kernel                  │
                 └──────────────────────────┘
```

Two crates cut across the stack rather than sitting in it:

- `whelk-job` — the job table, process groups, foreground/background transitions.
  Arrived in Phase 6.
- `whelk-terminal` — terminal modes, size, and ownership. Arrived in Phase 7,
  which is also when `tcsetpgrp` moved here from `whelk-process`: keeping every
  question about the terminal in one crate turned out to matter more than the
  argument that ownership is really about process groups.

`whelk-event` sits under the REPL: readiness notification over `epoll` and
`kqueue`, so the loop can wait on input and signals at once. It knows nothing
about shells.

A third, `whelk-line`, sits beside the REPL rather than under it: line editing,
history, and completion. It performs no I/O at all — keys in, an action out —
which is what lets a component that is almost entirely edge cases be tested
without a terminal.

They are separate because job control and terminal ownership are *not* a step in
the pipeline from text to process. They are long-lived state that both the REPL
and the executor consult — a job outlives the command that created it, which is
the whole reason it needs a table.

## Rules the layering enforces

**Parsing never touches the operating system.** `whelk-parser` produces an AST
from a string and can be tested exhaustively without spawning anything. A
parser that resolved `$PATH` or opened files would be untestable in the way
that matters.

**Execution never re-parses.** `whelk-executor` receives structure, not text. It
reads the source line for exactly one purpose — printing a caret under an error
— and the spans in the tree tell it which characters to underline, never what
they mean.

**Expansion is execution, not parsing.** `$HOME` is *recognised* by the lexer
and *resolved* by the executor. The split falls out of the first rule: reading
a variable is reading process state. Keeping it on the executor's side of the
line means the syntax of expansion is tested with no environment at all, and its
semantics are tested against a fake one — neither test needs a real process.
See [`parsing.md`](parsing.md).

**`whelk-process` owns every `unsafe` block that talks to the process table.**
`fork` and `exec` are the two calls whose *contract* is unsafe — not because
they might segfault, but because the window between them runs in a child
process where most of the language's assumptions have quietly stopped holding.
Concentrating them in one crate means the audit surface is one file, not the
whole shell.

## The dangerous window

Between `fork()` returning `0` and `execvp()` replacing the image, the child
may only call async-signal-safe functions. It has a copy of the parent's
address space, but any lock held by another thread at fork time is now held
forever by a thread that does not exist in the child.

`whelk` handles this by doing all fallible, allocating work — `PATH` resolution,
building the `argv` array of C strings — **before** the fork. The child's job
is reduced to `execvp` and, if that fails, `_exit`. It allocates nothing.

This is the single most important invariant in the codebase, and it is why
`whelk-process` exists as its own crate. See
[`docs/process-model.md`](process-model.md) for the details.

## Dependencies

`whelk` deliberately carries very few. The one that matters is [`nix`], a thin
wrapper over `libc` that adds Rust types (`Pid`, `WaitStatus`, `OwnedFd`)
without adding a runtime or a policy of its own.

This is not a contradiction of "systems first". Writing `libc::waitpid` by hand
does not teach anything `nix::sys::wait::waitpid` hides — the syscall semantics
are identical, and the only difference is who converts the `c_int` into a
`WaitStatus`. What `nix` does *not* do is spawn processes, manage jobs, or own
the terminal; that is exactly the work this project is here to write.

[`nix`]: https://docs.rs/nix
