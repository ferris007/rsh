# Redirection

How `echo hi > out.txt` becomes a file with `hi` in it, and why none of it
involves telling `echo` anything.

## The central idea

`echo` does not know about files. It writes to descriptor 1, exactly as it
always does. The shell arranged, before `echo` ever started, for descriptor 1
to *be* the file.

That is the whole mechanism. A program's output is not a destination it chooses;
it is a number it writes to, and somebody else decided what that number points
at.

```text
        shell, before fork              child, after dup2(3, 1)
   ┌────┬──────────────────┐       ┌────┬──────────────────┐
 0 │    │ terminal         │     0 │    │ terminal         │
 1 │    │ terminal         │     1 │    │ out.txt          │  ← moved
 2 │    │ terminal         │     2 │    │ terminal         │
 3 │    │ out.txt (CLOEXEC)│     3 │    │ (closed at exec) │
   └────┴──────────────────┘       └────┴──────────────────┘
```

## Where each step happens

| Step | Where | Why |
| --- | --- | --- |
| expand the target word | parent | needs the environment |
| open the file | parent | can fail, and must be reportable |
| check `>&N` is open | parent | same |
| `dup2` | child | must not disturb the shell's own descriptors |

The split is the same rule as `PATH` resolution in
[`process-model.md`](process-model.md): everything fallible happens before the
fork, so a missing file is an ordinary error message rather than an exit code
from a process that cannot explain itself.

What is left for the child is one `dup2` per redirection. `dup2` is
async-signal-safe and allocates nothing, which is what makes it legal in the
window between `fork` and `exec`.

## Order is the whole story

Redirections are applied left to right, and the AST preserves the order the
user wrote. This is not pedantry — it is the entire explanation of the most
famous confusion in shell usage:

```console
$ cmd > out.txt 2>&1        # both streams to the file
$ cmd 2>&1 > out.txt        # stderr to the terminal, stdout to the file
```

Read them as the `dup2` calls they become:

```text
> out.txt 2>&1                     2>&1 > out.txt
──────────────                     ──────────────
dup2(file, 1)   1 → file           dup2(1, 2)      2 → terminal
dup2(1, 2)      2 → file           dup2(file, 1)   1 → file
```

`2>&1` does not mean "send stderr to stdout". It means "make descriptor 2 a
copy of descriptor 1 *as it is right now*". Once you read it as a copy rather
than a link, the second form stops being surprising.

## Truncation happens at open time

`> file` opens with `O_TRUNC`, and the open happens while the redirection is
being *set up* — before the command is even looked up. Two consequences that
scripts rely on:

- `> lockfile` is a complete command. There is no program in it at all; its
  only effect is the redirection. (`rsh` does not accept this yet — it requires
  a command name — but the open-first ordering is the same.)
- `nosuchcommand > out.txt` creates `out.txt`, empty, and *then* reports that
  the command does not exist. Every POSIX shell does this. `rsh` has a test for
  it, because it looks like a bug and is not.

`>>` uses `O_APPEND`, which seeks to the end before every write, atomically.
Opening and seeking once would look equivalent and would interleave badly the
moment two processes share the file — which is what `cmd >> log &` does.

## Close-on-exec, and one sharp edge

The shell opens redirection targets with close-on-exec set (`std::fs` does this
for everything). `dup2` clears the flag on the copy it makes, so the child keeps
the redirected descriptor and the shell's original closes itself automatically
at `exec`. The program ends up with exactly the descriptors the user asked for
and no accidental extras.

Except when source and target are the same number:

```text
dup2(3, 4)  →  descriptor 4 is a fresh copy, close-on-exec clear
dup2(3, 3)  →  nothing happens at all, close-on-exec still set
```

POSIX says `dup2` with equal arguments returns the descriptor and performs no
other action — and "no other action" includes the flag clearing that was the
only reason to call it. `sh -c '...' 3> out.txt` hits this exactly: the file is
opened at descriptor 3, the lowest free one, and then asked to be descriptor 3.

The result is the worst kind of failure: no crash, no message, and a
redirection that silently does not happen. `rsh` handles it by calling
`fcntl(fd, F_SETFD, 0)` instead, which clears the flag directly and is also
async-signal-safe.

This is written up as a runnable experiment in
[`experiments/file_descriptors`](../experiments/file_descriptors/), because it
cost a real bug and reading past it in the man page is easy.

## Builtins are different

`cd` runs inside the shell. There is no child whose descriptors can be
arranged, so `cd - > where.txt` means the shell has to move *its own*
descriptor 1, run the builtin, and put it back.

Putting it back is not optional and not something to do at the end of a
function: a builtin that returns early would leave the shell writing into a
file forever. So the restore is a `Drop` guard, which is the only construct
that promises to run on every path out.

Two details that are easy to get wrong:

- **The saved copy goes to descriptor 10 or above**, with close-on-exec set, so
  it neither collides with a descriptor the user might redirect nor leaks into
  any child spawned while it is held.
- **Stdout is flushed twice** — before applying, so buffered output written
  earlier goes to the old descriptor, and before restoring, so the builtin's
  own output goes to the file. Rust's buffer is independent of the descriptor,
  and `dup2` knows nothing about it.

## What is not implemented

| Syntax | Status |
| --- | --- |
| `<<`, `<<-` | here-documents; needs multi-line input first |
| `>&-`, `<&-` | closing a descriptor |
| `>\|` | override noclobber, which does not exist yet |
| `<>` | open for reading and writing |
| `&>` | a bashism; `> f 2>&1` is the portable spelling |
