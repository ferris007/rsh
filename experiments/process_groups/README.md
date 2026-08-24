# `process_groups` — what happens when a background job reads from the terminal?

> **Question:** two processes, same terminal, both blocked in `read`. Only one
> of them gets the keystrokes. What happens to the other?

Anyone who has typed `cat &` has seen the answer without being told what it was:

```console
$ cat &
[1] 4242
$
[1]+  Stopped                 cat
```

It stops. Nobody stopped it, and it never read a byte.

## A race the demonstration had to close first

The children must not touch the terminal until the parent has finished deciding
who owns it. A child that reaches `read` before `tcsetpgrp` runs is not in the
foreground group *yet* — so it is stopped by `SIGTTIN`, including the one about
to be given the terminal.

This program lost that race on a CI runner and reported both children as
stopped, which looks like a demonstration of something it is not. The children
now wait at a pipe until the parent lets them through; reading a pipe is not
reading the terminal, and cannot earn a `SIGTTIN`.

A real shell has the same problem and solves it the same way round. It is
exactly why `fg` hands over the terminal *before* sending `SIGCONT`: a job
resumed first would read a terminal it does not own yet, stop again, and the
resume would appear to do nothing.

## The setup

The program forks two children, each leading its own process group, and both
running `cat` — so both block trying to read the terminal. Then it gives the
terminal to one of them and looks at what became of each.

It has to do this inside a session that *owns* a terminal, which is why it uses
`forkpty` rather than just opening a pty. `SIGTTIN` is only sent for a read from
a process's **controlling terminal**, and a process only has one if its session
acquired it. An earlier version of this experiment opened a pty, called
`setsid`, and observed nothing at all — both children read happily, because
neither had a controlling terminal for the rule to apply to.

## Running it

```console
$ cargo run -p xp-process-groups
two children, each in its own process group:
  1250870 and 1250871

gave the terminal to 1250870

  foreground: running, blocked in read — it owns the terminal
  background: stopped by SIGTTIN — it may not read
```

## Observation

The child holding the terminal blocks in `read`, as expected. The other is
**stopped by `SIGTTIN`** — a signal the kernel sent for the act of trying.

## What is going on

A terminal has exactly one **foreground process group**, set with `tcsetpgrp`.
Reading from a controlling terminal while not in that group is not an error and
not a race — it is a rule, enforced with a signal whose default action is to
suspend the offender.

```text
             ┌──────────────── terminal ────────────────┐
             │  foreground process group: 1250870       │
             └──────────────────────────────────────────┘
                     ▲                        ▲
                     │ read() → keystrokes    │ read() → SIGTTIN → stopped
              ┌──────┴──────┐          ┌──────┴──────┐
              │  pgid 1250870│          │ pgid 1250871│
              └─────────────┘          └─────────────┘
```

The reasoning is that two programs cannot sensibly share one keyboard. Rather
than interleaving keystrokes unpredictably, the kernel suspends the one that has
no claim and leaves it for a shell to resume — which is exactly what `fg` does.

`SIGTTOU` is the same rule for writing, applied only when the terminal's
`TOSTOP` flag is set. It is off by default, which is why background jobs can
scribble over your prompt but cannot steal your typing.

## Why this decides the shape of a shell

**`fg` is `tcsetpgrp` plus `SIGCONT`, in that order.** Continuing the job first
would let it read a terminal it does not own yet and be stopped again
immediately — the resume would appear to do nothing at all.

**The shell must ignore `SIGTTOU`.** `tcsetpgrp` is itself a terminal
operation, so a shell taking the terminal *back* from a job is a non-foreground
process touching the terminal. With the default action it would stop itself at
the exact moment the user pressed Ctrl-Z, leaving a frozen terminal and no shell
to unfreeze it.

**And it must not pass that on.** `SIG_IGN` is inherited across `exec`, so a
shell that ignores `SIGTSTP`, `SIGTTIN`, and `SIGTTOU` for itself has to reset
all three in every child, or its own self-protection silently becomes a property
of every program it runs. `whelk` does this in `Command::spawn` — the same trap
`SIGPIPE` sets in [`../pipes`](../pipes/), from the other direction.

## Going further

- Turn on `TOSTOP` with `stty tostop` and watch a background job stop the moment
  it tries to *write* as well.
- Check what happens to a stopped background job when its terminal goes away:
  `SIGHUP` reaches the session, and stopped processes get `SIGCONT` first so
  they are awake to receive it.
- Read the "Job Control" section of POSIX. It is shorter than it looks, and
  almost all of it is consequences of this one rule.
