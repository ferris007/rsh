# `signals` — what happens to a pipeline when the shell receives SIGINT?

> **Question:** you press Ctrl-C. Who actually gets the signal?

## The short answer

Not the shell, and not "the running command". The terminal driver sends
`SIGINT` to the **foreground process group** — every process in it, at once.

```text
   Ctrl-C
     │
     ▼
  terminal driver
     │  SIGINT to the foreground process group
     ▼
  ┌───────────────────────────────┐
  │  process group 4242           │
  │                               │
  │   cat  ──►  grep  ──►  sort   │   all three get it
  └───────────────────────────────┘
```

That is why one keystroke stops an entire pipeline without the shell doing
anything, and why nothing has to know how many stages there were.

## The setup

The program first moves *itself* into a new process group. Without that, the
group it signals below would be the one it was launched in — containing the
shell that started it, or `cargo test`. Running the experiment would interrupt
whatever was running the experiment.

That is not an artefact of the demonstration. It is the reason a shell puts
every job in a group of its own.

It then starts two children that do nothing but sleep. One is left in the
parent's process group; the other calls `setpgid` to move into a group of its
own. Then the parent signals its own group and reports what happened to each.

## Running it

```console
$ cargo run -p xp-signals
this process group: 1231693 (moved here, so nothing outside is signalled)

child in the same group: 1232274
child in its own group:  1232275

sending SIGINT to process group 1231693
  same group: killed by SIGINT
  own group:  still running — the signal never reached it
```

## Observation

Group membership is the entire difference. Same process id situation, same
program, same parent — one dies and one does not, because a group-directed
signal goes to a group.

## Why this decides the shape of a shell

**Ctrl-C reaches the whole pipeline.** No bookkeeping required. The stages were
put in one group, so one signal covers them.

**The shell must not die with them.** `rsh` is in that group too — it is the
process the terminal is talking to. So it installs a handler for `SIGINT`
rather than relying on being missed. Note the ordering in the program above:
the parent ignores the signal only *after* forking, because `SIG_IGN` is
inherited across `exec` and would have made both children immune. A real shell
uses a handler instead, which `exec` resets for free. Same trap as `SIGPIPE` —
see [`../pipes`](../pipes/).

**A background job needs its own group.** That is the second line of output:
put a job in its own group and Ctrl-C cannot touch it. This is not a special
case bolted on later — it is the same mechanism, used deliberately.

**Only one group can be in the foreground.** The terminal has exactly one
foreground process group, set with `tcsetpgrp`. Running a job in the foreground
*is* handing it that slot; suspending it is taking the slot back. Job control
turns out to be almost entirely this one piece of state.

## Where `rsh` is today

Phase 5 does the signal half: the shell handles `SIGINT`, `SIGTERM`, and
`SIGHUP`, survives Ctrl-C, reports 130, and notices a stopped child instead of
blocking on it forever.

It does **not** yet create process groups. Every child shares the shell's
group, which is why Ctrl-C reaches a foreground command at all right now — and
equally why there is no way to protect a background job from it. Phase 6 adds
`setpgid` and a job table; Phase 7 adds `tcsetpgrp` and the terminal side.

## Going further

- Note that `setpgid` is called in *both* parent and child in real shells. Either
  may run first after `fork`, and the child must be in its group before it can
  be signalled — so both do it and one call is redundant. Which one is
  redundant is not knowable in advance.
- Try `SIGSTOP` instead of `SIGINT` and watch the group stop together.
- Check what happens to a child in its own group when the *terminal* goes away:
  `SIGHUP` follows the session, not the group.
