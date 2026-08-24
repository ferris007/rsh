# Terminal management

A terminal is not a file, however much the descriptor pretends otherwise. It is
a device with modes, a size, and an owner — and, crucially, **state that
outlives the process that changed it**.

That last property is the whole of this phase. A program that puts the terminal
in raw mode and dies leaves it in raw mode. The shell is the only thing still
running that knows what it was before.

## Three things called "terminal"

| Concern | Set with | Question it answers |
| --- | --- | --- |
| Ownership | `tcsetpgrp` | Which process group may read it, and gets its signals? |
| Modes | `tcsetattr` | Does the driver echo, buffer lines, turn Ctrl-C into a signal? |
| Size | `TIOCGWINSZ` | How many rows and columns, right now? |

Ownership is [job control](job-control.md). This document is the other two.

## Canonical and raw

In **canonical** mode — the default — the terminal driver does a surprising
amount of work before a program sees anything:

- it buffers a whole line, and only returns from `read` when Enter is pressed;
- it handles backspace, Ctrl-U, and Ctrl-W itself;
- it echoes what is typed;
- it turns Ctrl-C into `SIGINT`, Ctrl-Z into `SIGTSTP`, Ctrl-D into end-of-file.

This is why `whelk` had working backspace and Ctrl-C from Phase 1, before a line
of code was written for either. The line editing you get for free is the
driver's.

In **raw** mode none of it happens. Every keystroke arrives immediately, nothing
is echoed, and Ctrl-C is the byte `0x03`. Editors need this; so does any shell
implementing its own history and arrow keys, which is Phase 8.

`whelk-terminal` provides raw mode now and the REPL does not use it yet. That is
deliberate: an untested capability adopted a phase later is a capability
debugged a phase later. It is exercised through a pseudoterminal, because a
function that operates on descriptor 0 needs a process whose descriptor 0 is a
terminal.

## The invariant

**Whatever a job does to the terminal, the shell puts it back before the next
prompt.**

```text
   start job ──► job changes modes ──► job ends or stops
                                            │
                            snapshot what the job left ──► store on the job
                                            │
                            restore the shell's own modes ──► prompt
```

Not because jobs are untrustworthy, but because a job suspended mid-`vim` has
legitimately left the terminal in a state the shell cannot use — and a job
*killed* mid-`vim` has left it that way with nobody to notice.

The familiar symptom is a terminal that stops echoing, and the usual remedy is
to type `reset` blind. A shell can do better, because it is still running.

`dash` does not do this. Feeding both shells `stty -echo` followed by another
command shows it directly: under `whelk` the second command is echoed as you type
it, under `dash` it is not.

## Resuming has to undo the undoing

A job stopped inside `vim` left the terminal raw. The shell put its own modes
back so it could print a prompt. `fg` therefore has to restore the *job's*
modes before handing the terminal over, or the editor comes back to a terminal
that echoes and buffers lines — visibly broken, and not the job's fault.

So a stopped job carries the modes it was using, and `fg` becomes:

```text
restore the job's modes  →  tcsetpgrp  →  SIGCONT  →  wait
```

The order is still the one job control requires; there is simply one more step
in front of it.

## `TCSADRAIN`, not `TCSANOW`

`tcsetattr` takes a flag saying when the change should take effect.
`TCSADRAIN` waits for pending output to be written first.

Changing modes out from under bytes still in the driver's queue is how output
ends up mangled at exactly the moment a program exits — which is the last thing
anyone wants to debug. The cost is a wait measured in milliseconds.

## Size, and why the shell cares

`COLUMNS` and `LINES` are environment variables, so **children read them**. A
shell that never updated them hands every program it runs a stale idea of the
window, and the symptom is `less` or `ps` formatting for the wrong width.

The kernel announces a resize with `SIGWINCH` to the foreground process group.
The handler does not read the new size: `ioctl` is not a call a handler may
make, and there is no hurry — the answer is only useful at the next prompt.

So the shell sets a flag in the handler and asks at the prompt, which means a
resize takes effect for the *next* command rather than the one already running.
That matches every other shell, and for the same reason.

## Restoration on the way out

The shell restores the terminal before exiting, including when it exits because
of `SIGTERM` or `SIGHUP`. The last job may well have been killed halfway
through changing something.

Restoration is a `Drop` guard wherever raw mode is entered, for the same reason
redirection uses one: the restore has to happen on every path out, including a
panic. A shell that returned early from an error while the terminal was raw
would leave the user with no echo and no working Ctrl-C.

## Not implemented

- **Alternate screen and cursor handling.** `whelk` does not draw; programs that
  do are on their own, which is correct.
- **Terminal capability queries.** No `terminfo`, no `TERM` interpretation.
  Phase 8 needs some of this for cursor movement.
- **`SIGWINCH` forwarding.** The kernel already delivers it to the foreground
  group, so a running job hears about resizes directly. A *stopped* job does
  not, and is not told when resumed.
- **PTY allocation.** `whelk` runs children on its own terminal. Allocating a pty
  per job is what `script`, `tmux`, and CI runners do, and it is a different
  program.
