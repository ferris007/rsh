# `fork_exec` — why the child must call `_exit`, not `exit`

> **Question:** a forked child that terminates with `exit()` instead of
> `_exit()` — what does it take with it?

`rsh` spawns every external command by forking and calling `execv`. If `execv`
fails, the child has to terminate, and `crates/rsh-process/src/command.rs` is
careful to use `_exit` rather than `exit` at that point. This experiment is why.

## The setup

The program writes a marker to stdout **without a trailing newline**, so the
text stays in the process's stdout buffer rather than reaching the file
descriptor. Then it forks. The child terminates immediately — one of two ways —
and the parent waits, then writes a newline, which flushes its own buffer.

The child writes nothing. It only exits.

## Running it

```console
$ cargo run -p xp-fork-exec -- exit
[buffered][buffered]
child terminated with Exit; count the markers on stdout

$ cargo run -p xp-fork-exec -- _exit
[buffered]
child terminated with UnderscoreExit; count the markers on stdout
```

(The second line of each run is on stderr, so it does not disturb the count.)

## Observation

With `exit()`, the marker appears **twice**. With `_exit()`, **once**.

## What happened

`fork` copies the parent's address space, and the stdout buffer is part of it.
The child inherits the bytes *and* the obligation to flush them:

```text
             parent                          child
             ──────                          ─────
 stdout buffer: "[buffered]"
 fd 1: (nothing written yet)
                    │
                  fork() ──────────────►  stdout buffer: "[buffered]"   ← copy
                    │                     fd 1: the same open file
                    │                              │
                    │                        exit()  → atexit handlers
                    │                              → flush stdio
                    │                              → write(1, "[buffered]")
                    │                                       │
              waitpid() ◄──────────────────────────────────┘
                    │
             println!() → flush → write(1, "[buffered]\n")
```

`exit(3)` is a C library function. It runs the handlers registered with
`atexit`, and one of the things a language runtime registers there is "flush
the standard streams". The child had no idea what was in that buffer — it never
wrote a byte of it — but it flushed it anyway, because flushing is an
unconditional part of exiting cleanly.

`_exit(2)` is a syscall. It returns to the kernel without running any handler,
so the inherited buffer dies with the address space.

Note also that the two processes share the *open file description*, not just
the descriptor number, which is why both writes land in the same stream at the
same offset instead of overwriting each other.

## Why it matters to the shell

Two independent consequences for `rsh`:

1. **The failure path must use `_exit`.** When `execv` fails, the child has to
   terminate without dragging the parent's pending output out with it.
2. **The parent must flush before forking.** `rsh-executor` calls
   `stdout().flush()` before every spawn for exactly this reason: an empty
   buffer cannot be duplicated. This is belt-and-braces with (1), and it is
   worth having both, because a child that successfully `exec`s a *different*
   program is beyond our control — that program's runtime will do whatever it
   does on exit, including flushing the buffer it inherited from us.

The second point is the one that is easy to miss. `_exit` protects the failure
path; flushing early protects the success path.

## A wrinkle worth knowing

This reproduces reliably here because Rust's stdout is line-buffered whether or
not it is a terminal, so an unterminated line stays in the buffer in both
cases. C stdio is *fully* buffered when stdout is not a terminal and
line-buffered when it is — so the same experiment written in C shows the
duplication when piped to a file and hides it at a terminal, which is a
memorably nasty way to meet this bug.

## Going further

- Run it under `strace -f -e trace=write` and watch two `write(1, ...)` calls
  appear in the `exit` case and one in the `_exit` case.
- Replace `print!` with `eprint!` and observe that stderr, being unbuffered,
  never duplicates.
- Have the child `execv` something instead of exiting, and check whether the
  inherited buffer survives the image replacement.
