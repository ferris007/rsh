# Events

Until Phase 9 the shell waited for exactly one thing at a time: `read` on
standard input, or `waitpid` on a child. That works because it always knew which
one it was waiting for.

It is also why a background job finishing went unnoticed until the next prompt,
and why dragging the window did nothing until the user pressed Enter. The shell
was asleep on one descriptor with no way to hear about anything else.

An event loop inverts the question. Instead of "wait for this", the shell says
"wake me when **any** of these has something to say", and is told which.

```text
              ┌──────────────┐
              │    Poller    │   epoll on Linux, kqueue on BSD and macOS
              └──────┬───────┘
         ┌───────────┼───────────┐
         ▼           ▼           ▼
       stdin      signals    (later: children, timers)
```

## What changed for the user

Very little, and that is worth saying plainly. This phase is an architectural
change, not a feature.

Two things did get better:

- **A resize takes effect immediately.** Dragging the window while typing
  redraws the line at its new width, and `COLUMNS` is updated in time for the
  command being typed — not the one after it.
- **Children are reaped promptly**, rather than accumulating for the length of
  an editing session.

The rest is groundwork. A shell that can wait on several sources is a shell that
*could* have timers, an inotify watch on a prompt's git status, or a completion
that reads a slow filesystem without freezing the line — none of which exist,
and all of which were previously impossible to add.

## This crate is a very small `mio`

`rsh-event` is about two hundred lines: register a descriptor, wait, get tokens
back. That is the layer Tokio is built on.

Writing it is the point. Your roadmap says not to reach for Tokio before
understanding what it abstracts, and the honest answer is that an async runtime
is a *scheduler* on top of exactly this. Futures, wakers, and executors are all
above the line; below it is a poller telling you which descriptors are ready.
The scheduler is much easier to reason about once the thing underneath is not
mysterious.

## Two backends, one narrow interface

`epoll` and `kqueue` solve the same problem and disagree about nearly everything
else:

| | `epoll` | `kqueue` |
| --- | --- | --- |
| register | `epoll_ctl`, a separate call | a "change" submitted to `kevent` |
| wait | `epoll_wait` | `kevent`, the same call |
| user data | a `u64` in the event | `udata`, a pointer-sized field |
| far end closed | `EPOLLHUP` alongside `EPOLLIN` | `EV_EOF` flagged on the read event |
| timeout | milliseconds as an `int` | a `timespec` |

The shared surface is deliberately narrow, because every capability one backend
has and the other does not is a capability the shell cannot use portably. A
poller that exposed them would push the platform difference into the caller,
which is precisely the thing an abstraction is for avoiding.

## Level-triggered, not edge-triggered

Readiness persists until the data is read. Ask twice without reading and you are
told twice.

Edge triggering reports the *change* instead, which is faster under load and
requires the caller to drain every descriptor completely on every wake — because
anything left behind will never be announced again. That is a correctness
obligation on every reader in the program, in exchange for a saving at an event
rate a shell will never approach.

## Signals are not descriptors

Neither `epoll` nor `kqueue` can portably wait for a signal, and the obvious
workaround — set a flag in the handler, check it in the loop — has a race:

```text
    if flag.take() { handle it }     ← clear; nothing to do
                                     ← the signal arrives. handler sets the flag
    poller.wait(forever)             ← blocks, with nothing left to wake it
```

The signal *was* handled. The flag is set, correctly. But the loop had already
decided there was nothing to do, and no event remains to return from the wait.

The fix is the **self-pipe trick**: the handler writes one byte to a pipe the
poller is watching. A flag is a fact about the present; a byte in a pipe is a
fact that waits. The race disappears not because the timing changed but because
the event was made durable.

[`experiments/epoll`](../experiments/epoll/) reproduces the race
deterministically — 800ms slept through with a flag alone, 0ms with the pipe.

Three details that matter:

- **The write end is non-blocking.** A handler firing faster than the loop
  drains would otherwise block *inside a signal handler*, with no way out. A
  full pipe means a wakeup is already pending, so the failed write means the job
  is already done.
- **Both ends are close-on-exec.** Children have no business inheriting the
  shell's wakeup channel.
- **The bytes carry no meaning.** A handler cannot safely encode which signal it
  was and does not need to: the flags already say. The pipe's only job is to
  make the poller return.

`signalfd` on Linux and `EVFILT_SIGNAL` on BSD both do this properly, and both
are platform-specific. The self-pipe needs nothing but a pipe.

## Standard input stays blocking

The roadmap lists "non-blocking I/O", and the shell deliberately does not set
`O_NONBLOCK` on descriptor 0.

`O_NONBLOCK` is a property of the **open file description**, not of the
descriptor. Setting it on standard input sets it for every process sharing that
description — which is every child the shell starts. A `cat` that suddenly gets
`EAGAIN` from a terminal is a bug the shell caused and the user cannot explain.

The poller makes it unnecessary anyway: a read performed only after readiness
has been reported returns immediately whether or not the descriptor is blocking.
Non-blocking mode is used where the shell *owns* the descriptor outright — the
self-pipe — and nowhere else.

## Who consumes an event

A flag taken is a flag gone, so whoever takes it must handle it completely.

The resize flag has two readers: the main loop, for a window dragged while a
command ran, and the line editor, for one dragged mid-line. Each does the whole
job — update `COLUMNS` and `LINES`, then redraw if there is a line to redraw.
A consumer that did half the work would leave the environment stale depending
on *when* the user happened to drag the window, which is the kind of bug that
gets reported as "sometimes".

## Not implemented

- **Children as events.** `SIGCHLD` still arrives as a signal and is collected
  with `waitpid`. Linux's `pidfd` would make a child a descriptor like anything
  else; there is no portable equivalent.
- **Timers.** `timerfd` and `EVFILT_TIMER` are both platform-specific; a
  timeout argument to `wait` covers what the shell needs.
- **The executor.** Pipelines still block in `waitpid`. Moving them onto the
  loop is what would let the shell stay responsive while a foreground job runs,
  and it is a larger change than this phase.
- **Edge triggering, `EPOLLEXCLUSIVE`, and the rest.** Not needed at a shell's
  event rate.
