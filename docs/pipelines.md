# Pipelines

`a | b | c` is three processes and two pipes, all alive at once.

That is the part worth internalising. A pipeline is not a sequence of steps with
the output of one handed to the next — it is a set of concurrent processes
connected by kernel buffers. `c` starts reading before `a` has finished writing,
which is why this answers instantly:

```console
whelk> find / | head -1
```

`find` is still walking the filesystem when `head` prints its line and exits.
Nothing buffered the disk.

## What the shell builds

```text
          pipe 0              pipe 1
    a ──────────────► b ──────────────► c
    │                 │                 │
    └─ stdout is      └─ stdin is       └─ stdin is
       pipe 0's          pipe 0's read     pipe 1's read
       write end         stdout is
                         pipe 1's write
```

For *n* commands: *n* − 1 pipes, *n* forks, *n* waits. The first stage keeps the
shell's stdin and the last keeps its stdout, which is how a pipeline stays
connected to the terminal at both ends.

## The rule everything depends on

**A reader sees end-of-input only when every write end of its pipe is closed.**
Not most of them — all.

This is the source of essentially every pipeline hang. Each child inherits a
copy of *every* pipe in the pipeline, not just its own, and the shell holds
copies too. A single forgotten descriptor anywhere keeps a reader waiting for
data that will never come.

`whelk` never closes a pipe descriptor by hand. Two mechanisms do it:

- **In children**: pipe ends are close-on-exec. `dup2` clears the flag on the
  one or two a child was given, and `exec` closes everything still carrying it.
  A child that needs nothing from a pipe does nothing about it.
- **In the shell**: the pipes are owned by a local variable, and dropping it
  after the last fork closes every end at once.

Neither is clever. Both are chosen because the alternative — a list of closes in
the child, where a mistake is invisible — is the kind of code that works until
someone adds a fourth stage.

## Backpressure

Writes block when the buffer is full. On Linux the buffer is 64 KiB by default:

```console
$ # write to a pipe nobody is reading, until write() blocks
pipe capacity before writes block: 65536 bytes (64 KiB)
```

So `yes | head -1` does not generate infinite output and then discard it. `yes`
fills 64 KiB, blocks, and waits — the consumer's speed limits the producer, for
free, with no coordination between them. Flow control in a shell pipeline is
entirely this one property.

`F_SETPIPE_SZ` adjusts the buffer on Linux, which is occasionally the answer to
a throughput problem and much more often a sign that the pipeline should not
have been a pipeline.

## Broken pipes, and a bug Rust hands you

When the reader exits first, the writer is writing into a pipe with no reader.
The kernel sends it `SIGPIPE`, whose default action is to kill it. That is how
`yes | head` ends: nobody coordinates, the producer simply dies once the
consumer leaves.

A shell written in Rust breaks this by default.

Rust's runtime sets `SIGPIPE` to `SIG_IGN` before `main`, so a Rust program sees
`EPIPE` from `write` instead of dying. Reasonable for a Rust program. The
problem is that `exec` **keeps** `SIG_IGN` — it resets *handlers*, because the
new program does not have the old one's code, but a disposition that is not a
handler survives. So every program the shell runs would inherit "ignore
SIGPIPE".

The result is not a crash. The pipeline still produces correct output; it just
loses the mechanism that stops the producer, and what happens instead depends on
how carefully a program written decades ago handled a write error.

`whelk` resets `SIGPIPE` to `SIG_DFL` in the child, between `fork` and `exec`.
Demonstrated in [`experiments/pipes`](../experiments/pipes/).

## Exit status

The pipeline's status is the **last** command's:

```console
whelk> sh -c 'exit 3' | sh -c 'exit 0'
whelk> echo $?
0
```

That is POSIX, and it is why `grep -q pattern | true` succeeds regardless of
what `grep` found. Reporting the first failure would be more useful and less
compatible; `bash` offers both through `set -o pipefail`, which is a later
problem.

Every child is waited for, not just the last. A shell that reaped only the
process whose status it needed would leave a zombie per pipeline stage for the
rest of the session.

## Redirections override pipes

```console
whelk> echo hi > f.txt | cat
```

`cat` receives nothing and `f.txt` contains `hi`. The pipe is wired up first and
the command's own redirections are applied afterwards, so the later `dup2` wins.
This falls out of applying them in order and is worth a test rather than a
comment.

## Known divergences

**A pipeline that cannot be set up runs nothing.** Every stage is prepared —
expanded, resolved, files opened — before anything is forked. If a redirection
fails on stage 3, `bash` still runs stages 1 and 2 and lets stage 3 report the
error; `whelk` reports the error and runs none of it. Preparing first is what
makes "no half-started pipeline" a property rather than a hope; the trade is
this difference, which is visible only when a stage's redirection fails.

**Builtins cannot appear in a pipeline.** `cd /usr | cat` is refused. A builtin
changes the shell's own state, so running one in a pipeline means forking a
subshell — real machinery that arrives with Phase 6. The alternative, letting
`cd` in a pipeline move the actual shell, is something no other shell does and
would be worse than refusing.

**No `pipefail`, no `PIPESTATUS`.** Only the last status is kept.
