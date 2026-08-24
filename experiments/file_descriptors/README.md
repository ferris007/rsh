# `file_descriptors` — what does `dup2(fd, fd)` do?

> **Question:** `dup2` normally clears close-on-exec on the descriptor it
> writes to. What happens when the source and the target are the same number?

This experiment exists because the answer caused a real bug in `rsh`, in the
commit that added redirection.

## The setup

A shell that runs `sh -c '...' 3> out.txt` has to:

1. open `out.txt` — landing on descriptor 3, the lowest free one, with
   close-on-exec set, because that is what `std::fs` does; and
2. arrange for the child's descriptor 3 to be that file.

Step 2 is `dup2(3, 3)`. It looks like a no-op, and the obvious optimisation is
to skip it when source and target match.

That optimisation is wrong, and the way it is wrong is silent.

## Running it

```console
$ cargo run -p xp-file-descriptors -- /tmp/target.txt
opened as fd 3, FD_CLOEXEC: set
after dup2(3, 3) -> 3, FD_CLOEXEC: set
after dup2(3, 4) -> 4, FD_CLOEXEC: clear
exec with the flag left as-is:
/bin/sh: 1: 3: Bad file descriptor
  child failed with 2: the descriptor was closed by exec
exec with the flag cleared first:
  child succeeded: the descriptor survived exec
file now contains: "written-to-3\n"
```

## Observation

`dup2(3, 4)` clears close-on-exec on the new descriptor. `dup2(3, 3)` does
not — it does nothing at all.

POSIX is explicit about this: if the two arguments are equal, `dup2` returns
the descriptor *without* performing any other action. "Any other action"
includes the flag clearing, which is the only reason the shell was calling it.

## What happened

Close-on-exec is a property of the **descriptor**, not of the open file behind
it. That is why a fresh copy never has it:

```text
      before exec                          after exec
  ┌────┬──────────────┐               ┌────┬──────────────┐
3 │ ✗  │ out.txt      │  CLOEXEC set  │    │  (closed)    │
4 │    │ out.txt      │  CLOEXEC clr  │ 4  │ out.txt      │
  └────┴──────────────┘               └────┴──────────────┘
        ▲                                    ▲
        │ same open file, two descriptors, different flags
```

`dup2(3, 4)` makes descriptor 4 refer to the same open file with a clear flag.
`dup2(3, 3)` has no copy to make, so there is no fresh flag either.

At `exec` the kernel closes every descriptor whose flag is set. The child then
runs `echo ... >&3` against a descriptor that is no longer there, and fails
with `EBADF`.

## Why it matters to the shell

The failure mode is the bad kind: nothing crashes, no error reaches the user,
and the redirection simply does not happen. `3> out.txt` produced an empty
file, and the child's error message went wherever stderr happened to point.

`rsh` handles it in `crates/rsh-process/src/redirect.rs`: when source and
target are equal it calls `fcntl(fd, F_SETFD, 0)` instead of `dup2`, which
clears the flag directly. `fcntl` is async-signal-safe, so it is legal in the
window between `fork` and `exec` where this code runs.

Redirections onto 0, 1, and 2 never hit this, because those descriptors are
already open and never the target of a fresh `open`. Only `3>`, `4>`, and
friends do — which is exactly why it survived the first round of testing.

## Going further

- Change `OpenOptions` to open without close-on-exec and watch the `skip` case
  start working, for the wrong reason: now *both* descriptors leak into the
  child.
- Check what `dup3(fd, fd, O_CLOEXEC)` does with equal arguments. Linux returns
  `EINVAL` rather than silently doing nothing — arguably the better design.
- Look at `/proc/<pid>/fd` in the child, before and after, to see the closure
  happen.
