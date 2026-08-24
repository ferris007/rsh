# `pty` — why does a program's output change when you pipe it?

> **Question:** a program prints progress as it works. Pipe it into `grep` and
> the progress stops appearing until the program finishes. Nothing was lost —
> where did it go?

## The setup

The program under observation prints a line, sleeps for a second, prints
another, and exits. It uses C's `printf`, because the rule at issue belongs to
C's standard library.

The driver runs it twice — once with its output on a pipe, once on a
pseudoterminal — and checks what has arrived after 400 milliseconds, well
inside the pause.

## Running it

```console
$ cargo run -p xp-pty --bin xp-pty
running: xp-pty-writer, which prints a line, sleeps, prints another
checking what has arrived after 400ms

with output on a pipe:
  after the pause: nothing yet
  in the end:      "first\nsecond\n"

with output on a pseudoterminal:
  after the pause: "first\n"
  in the end:      "first\nsecond\n"
```

## Observation

Same program, same output in the end, completely different timing.

On a terminal the first line arrives during the pause. On a pipe **nothing**
arrives until the program exits.

## What is going on

C's standard library picks a buffering mode the first time a stream is used,
and it picks by asking `isatty`:

| stdout is | buffering | flushed when |
| --- | --- | --- |
| a terminal | line buffered | every `\n` |
| anything else | fully buffered | the buffer fills, or at exit |

The reasoning is throughput. A program writing to a file or a pipe is usually
being consumed by another program, so batching into 4 KiB writes is a large win
and nobody is watching. A program writing to a terminal has a human waiting, so
latency matters more than syscall count.

The trouble is that the decision is invisible, it is made by a library the
program never mentions, and it changes behaviour based on how the program was
*invoked* rather than what it does.

## Consequences people actually hit

- **`prog | grep pattern` appears to hang.** `prog` is running fine; its output
  is sitting in a buffer inside `prog`.
- **A crashed program loses its last output.** The buffer dies with the
  process — nothing flushed it. Redirect the same program to a terminal and the
  output is all there, which makes the bug look like it depends on redirection.
- **`stdbuf -oL prog | grep` fixes it**, by preloading a library that overrides
  the choice. That such a tool exists is the strongest evidence this rule
  surprises people.

## What Rust does

Nothing like this. `std::io::Stdout` is a `LineWriter` whatever it is attached
to, so a Rust program behaves the same piped or not. That is a deliberate
departure from C, and one of the few places Rust quietly fixes a decades-old
footgun.

It is also why this experiment ships its own writer in C's terms: a Rust
`println!` would have shown no difference at all, and the absence would have
looked like the experiment was broken.

The flip side appears in [`../fork_exec`](../fork_exec/): because Rust's stdout
is *always* buffered, an unterminated line stays in the buffer even when output
is a terminal — which is exactly what makes a forked child duplicate it.

## Why it matters to the shell

A shell decides what a program's stdout is attached to. Every pipeline it builds
silently switches every stage from line buffering to full buffering, and the
user sees a program that "stopped working" when nothing of the sort happened.

`whelk` cannot fix this — the choice is made inside each child, by a library the
shell does not control. But it is worth knowing that the shell is the thing that
causes it.

## Going further

- Run the writer under `stdbuf -o0` and watch the pipe case start behaving.
- Check `setvbuf(3)`: a program can choose for itself, and well-behaved
  long-running tools do.
- Try the same experiment with stderr, which C leaves unbuffered in both cases —
  which is why error messages arrive when you expect them and normal output does
  not.
