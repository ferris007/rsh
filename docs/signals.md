# Signals

What Ctrl-C actually does, and why a signal handler can do almost nothing.

## The constraint

A handler runs by interrupting whatever the process was doing. That may be the
middle of `malloc`, holding the allocator's lock. If the handler allocates, it
waits for a lock its own thread already holds, and the process stops forever.

POSIX allows only [async-signal-safe] functions in a handler — the same rule as
the window after `fork`, for a related reason. In Rust that rules out
`println!`, formatting, allocation, locks, and anything that might reach one of
those a few calls down.

So `rsh`'s handlers do exactly one thing: store into a `static` atomic and
return.

```rust
extern "C" fn on_interrupt(_signal: c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}
```

A relaxed store to an `AtomicBool` is a single instruction — no lock, no call.
The shell reads the flag at points where it is safe to do real work. This
"handler records, main loop decides" shape is not a stylistic choice; it is what
the constraint leaves.

[async-signal-safe]: https://man7.org/linux/man-pages/man7/signal-safety.7.html

## Handler or `SIG_IGN`?

The shell needs Ctrl-C not to kill it. Ignoring `SIGINT` would achieve that in
one line. It installs a handler instead, and the reason is inheritance:

| | survives `exec`? |
| --- | --- |
| a handler | **no** — reset to the default action |
| `SIG_IGN` | **yes** |

`exec` replaces the program, so a handler's function pointer would be
meaningless and the kernel resets it. But "ignore" needs no code, so it is kept.

A shell that ignored `SIGINT` would therefore hand every child a program that
cannot be interrupted. With a handler, each child starts with the default action
and the shell does no extra work. This is the same trap `SIGPIPE` sets, from the
other direction — there, Rust's inherited `SIG_IGN` is the problem, and the
shell must reset it explicitly. See [`experiments/pipes`](../experiments/pipes/).

## Who receives Ctrl-C

Not the shell specifically. The terminal driver sends `SIGINT` to the
**foreground process group** — every process in it.

```text
   Ctrl-C
     │
     ▼
  terminal driver
     │  SIGINT to the foreground process group
     ▼
  ┌───────────────────────────────┐
  │   cat  ──►  grep  ──►  sort   │   all three, at once
  │   ...and rsh, which is in the │
  │   same group today            │
  └───────────────────────────────┘
```

One keystroke stops an entire pipeline with no bookkeeping, because the stages
share a group. Demonstrated in
[`experiments/signals`](../experiments/signals/).

`rsh` does not create process groups yet. Every child shares the shell's, which
is *why* Ctrl-C reaches a foreground command at all right now — and equally why
there is no way to shield a background job from it. Phase 6 adds `setpgid`.

## `SA_RESTART`, and why it is off

A signal that arrives during a blocking `read` either interrupts it — `EINTR` —
or, with `SA_RESTART`, resumes it transparently once the handler returns.

`SA_RESTART` is the convenient default for most programs and useless here. If
the read resumed, the shell would never learn that Ctrl-C happened while the
user was typing; the half-finished line would still be sitting there. Abandoning
that line *is* the response to Ctrl-C.

This has a consequence for how the shell reads input. `BufRead::read_line`
treats `EINTR` as "try again" and loops internally, which hides exactly the
event the shell needs. So `rsh` reads the descriptor directly, through a
`File` over descriptor 0 — `File::read` is a bare `read(2)` and reports `EINTR`
— and does its own line buffering.

## What each signal does

| Signal | The shell | Children |
| --- | --- | --- |
| `SIGINT` | records it; abandons the line, reports 130, prompts again | default action, so a foreground command dies |
| `SIGQUIT` | records it; same as above | default action |
| `SIGTERM` | shuts down after the current command, exits 143 | not forwarded |
| `SIGHUP` | shuts down, exits 129 | not forwarded |
| `SIGPIPE` | Rust's `SIG_IGN` | **reset to default**, or pipelines break |
| `SIGTSTP` | sees the stop, continues the child | default action |

Ctrl-C at the prompt sets `$?` to 130 even though nothing ran, because that is
what every other shell reports and `echo $?` should not lie about it.

## Stopped children, and a hang worth knowing about

`waitpid` does not return for a child that has been *stopped* rather than
terminated — not unless you ask, with `WUNTRACED`.

Before Phase 5, `rsh` did not ask. Pressing Ctrl-Z on a foreground command left
the child suspended and the shell blocked forever on a process that would never
finish: no prompt, no output, no way back short of killing the shell from
another terminal.

Having *seen* the stop, a shell has two options: keep the job somewhere and take
the terminal back, or continue the child and go on waiting. The first is job
control, and it needs a job table, process groups, and terminal ownership —
Phases 6 and 7. Until then `rsh` does the second, and says so:

```console
rsh> sleep 30
^Z
rsh: sleep: stopped by SIGTSTP; continuing (job control is phase 6)
```

### An aside worth knowing

`SIGTSTP`, `SIGTTIN`, and `SIGTTOU` are **discarded** when sent to a member of
an orphaned process group — a group where no member has a parent in a different
group of the same session. The kernel's reasoning is that there would be nobody
left able to resume it.

This is easy to trip over in tests. Under a CI runner the shell's process group
is orphaned, so a test that stops a child with `SIGTSTP` quietly stops testing
anything: the signal vanishes, the child runs to completion, and the assertions
about its output still pass. `rsh`'s tests use `SIGSTOP`, which cannot be
caught, blocked, or discarded.

Continuing is not the right long-term answer. It is the right answer while there
is nowhere to put a stopped job, because the alternative is stranding a process
that nothing is able to resume.

## Graceful shutdown

`SIGTERM` and `SIGHUP` set a flag rather than exiting from the handler. The
shell notices at the top of its loop and leaves with `128 + signal`.

Two details:

- The flag is **not** cleared when read. A shutdown request does not expire, and
  a shell that forgot one because it happened to check twice would keep running
  after being told to stop.
- End of input and a shutdown request can arrive together — a terminal hanging
  up does both. Being asked to terminate is the more specific fact, so it wins;
  reporting the last command's status there would lose the reason the shell
  stopped.

## Not implemented

**Forwarding signals to children.** `SIGTERM` to the shell does not terminate a
running foreground job.

**`SIGWINCH`.** Terminal resize, Phase 7.

`SIGCHLD` arrived in Phase 6, where background jobs finally gave it something to
report. See [`job-control.md`](job-control.md).
