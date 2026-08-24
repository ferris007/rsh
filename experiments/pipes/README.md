# `pipes` — what does a child inherit when the parent ignores a signal?

> **Question:** a Rust program ignores `SIGPIPE` before `main` runs. What
> happens to the programs it `exec`s?

This experiment exists because the answer is a bug that a shell written in Rust
gets for free, and it is invisible until someone runs `yes | head`.

## The setup

`exec` resets **handlers** to the default action — the new program has none of
the old program's code, so a function pointer would be meaningless. But
dispositions that are not handlers are *kept*: a signal set to `SIG_IGN` stays
ignored across `exec`.

Rust's runtime sets `SIGPIPE` to `SIG_IGN` at startup, before `main`. That is a
good default for a Rust program, which would rather see an `EPIPE` error from
`write` than die. It is inherited by every child.

The program below execs `sh -c 'kill -PIPE $$'` twice — once with the inherited
disposition, once after resetting it — so the answer shows up in the exit status
without needing a pipe or any timing.

## Running it

```console
$ cargo run -p xp-pipes
this process ignores SIGPIPE: true
(Rust's runtime sets that at startup, before main)

child with the inherited disposition:
  exited normally with 0 — the signal was ignored
child after resetting SIGPIPE to SIG_DFL:
  killed by SIGPIPE — the default action reached it
```

## Observation

The child inherits `SIG_IGN`. Asking to be killed by `SIGPIPE` does nothing at
all, and the process exits 0.

Compare the two shells directly:

```console
$ /bin/sh -c "sh -c 'kill -PIPE \$\$'"; echo $?
141                                   # 128 + SIGPIPE

$ whelk                                 # before the fix
whelk> sh -c 'kill -PIPE $$'
whelk> echo $?
0
```

## Why it matters to the shell

`yes | head -1` works because of SIGPIPE. `head` reads one line and exits,
closing its end of the pipe; `yes` writes to a pipe with no reader; the kernel
kills it. Nobody has to notice or coordinate — the producer dies because the
consumer left.

With `SIG_IGN` inherited, that mechanism is gone. `yes` gets `EPIPE` from
`write` instead, and what happens next is entirely up to how carefully somebody
handled a write error in a program written thirty years ago. GNU `yes` exits.
Something in a shell script's `while` loop may not.

The failure is quiet in the worst way: the pipeline still produces the right
output, so nothing looks wrong until a producer that ignores write errors spins
forever.

## The fix

`whelk` resets `SIGPIPE` to `SIG_DFL` in the child, between `fork` and `exec`,
in `crates/whelk-process/src/command.rs`. `signal` is async-signal-safe, and
setting a *disposition* rather than a handler means no Rust code can end up
running in a signal context.

The same reasoning will apply to `SIGINT` and `SIGQUIT` in Phase 5, where the
shell will ignore them for itself and has to be careful not to hand that
inheritance to its children.

## Going further

- Check `SIGCHLD`: `SIG_IGN` on it has a second, stranger meaning — children
  are reaped automatically and `wait` fails with `ECHILD`.
- Find the pipe buffer size by writing to a pipe nobody reads until `write`
  blocks. On Linux it is 64 KiB by default and adjustable with `F_SETPIPE_SZ`.
  That buffer is what backpressure is made of.
- Read the "Signals" section of `execve(2)` and note which properties survive:
  dispositions do, handlers do not, the signal mask does, pending signals do.
