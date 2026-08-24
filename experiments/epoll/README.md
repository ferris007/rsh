# `epoll` — why is a flag set by a signal handler not enough?

> **Question:** the handler sets a flag, the loop checks the flag. What is
> missing?

A signal handler may do almost nothing — no allocation, no locks, no
formatting. Setting a flag and returning is the standard shape, and every
description of signal handling arrives at it.

It is also not enough, and the reason is a race that is a few microseconds wide.

## The gap

```text
    if flag.take() { handle it }     ← flag is clear, nothing to do
                                     ← the signal arrives here.
                                        the handler runs and sets the flag
    poller.wait(forever)             ← blocks. nothing is left to wake it
```

The signal was handled — the flag is set, and correctly. But the loop had
already decided there was nothing to do, and there is now no event left to
return from the wait. The shell sleeps holding a fact it will not look at again
until something *else* happens to arrive.

## The setup

The race is normally impossible to hit on purpose. This program makes it
deterministic by **blocking** the signal, raising it so it is pending, and
unblocking it at the exact moment between the check and the wait.

Two rounds, differing in one line: whether the handler also writes a byte to a
pipe the poller is watching.

## Running it

```console
$ cargo run -p xp-epoll
a signal arrives between checking the flag and starting the wait
the loop then waits up to 800ms for something to happen

with a flag alone:
  flag before the wait: false
  the wait returned after 800ms
  nothing arrived: the loop slept through a signal it had already handled

with a self-pipe as well:
  flag before the wait: false
  the wait returned after 0ms
  woken by the pipe — the signal was not missed
```

## Observation

With a flag alone the loop waits the full timeout, having already handled the
signal. With a self-pipe it returns immediately.

## Why the pipe fixes it

A flag is a fact about the present. A byte in a pipe is a fact that **waits**.

```text
   handler:  flag = true;  write(pipe, "!")
                                  │
                                  └─► the byte is in the pipe, whenever
                                      the loop gets around to looking
   loop:     poller.wait(...)  ──────► returns immediately, because
                                       the pipe is readable
```

The handler writes before the wait begins, so the wait begins with something
already there. The race disappears — not because the timing changed, but
because the event was made durable.

This is the **self-pipe trick**, and it is what portable event loops do with
signals. The bytes carry no information: a handler cannot safely encode which
signal it was, and does not need to, because the flag already says.

## Details that matter

**The write end is non-blocking.** A handler firing faster than the loop drains
would otherwise block *inside a signal handler*, with no way out. A full pipe
means a wakeup is already pending, so the failed write means the job is done.

**Both ends are close-on-exec.** Children have no business inheriting the
shell's wakeup channel.

**The read end is drained, not interpreted.** However many bytes are in it, the
flags say what happened.

## What about the alternatives?

- **`signalfd`** turns signals into a readable descriptor directly, and is
  exactly what one wants — on Linux, which is where it exists.
- **`kqueue`'s `EVFILT_SIGNAL`** does the same thing on BSD and macOS, with a
  different interface.
- **`pselect`/`ppoll`** take a signal mask atomically, closing the same race a
  different way. `pselect` is POSIX; `ppoll` is not.

`whelk` uses the self-pipe because it is the one answer that works everywhere and
needs nothing from the platform beyond a pipe. See
[`docs/events.md`](../../docs/events.md).

## Going further

- Remove the `sigprocmask` calls and run it in a loop until the race happens on
  its own. It will, eventually, which is the worst property a bug can have.
- Try `signalfd` on Linux and see how much simpler the loop becomes when the
  signal *is* a descriptor.
- Note that `epoll_wait` returns `EINTR` when a signal is delivered *during* the
  wait. That handles half the problem, and it is the half that was never the
  difficulty.
