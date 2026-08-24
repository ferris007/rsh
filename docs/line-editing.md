# Line editing

The interactive layer: raw mode, history, completion, and the editor that ties
them together. Deliberately the phase furthest from the process core, and
deliberately the one with the least to say about Unix.

## What the terminal already does

Before any of this existed, `rsh` had working backspace, Ctrl-U, Ctrl-W,
Ctrl-C, and Ctrl-D. None of it was implemented. In **canonical** mode the
terminal driver buffers a line, handles the erase keys itself, echoes what is
typed, and turns Ctrl-C into a signal — a program's `read` only returns when
Enter is pressed.

Line editing means giving all of that up. In raw mode the driver does nothing:
every keystroke arrives immediately, nothing is echoed, and Ctrl-C is the byte
`0x03`. Everything the user sees from then on, the shell drew.

That is worth stating plainly, because it changes the cost of the feature. The
shell is not adding history to an existing editor; it is replacing the
terminal's editor with its own, and has to reimplement everything the old one
did before it can add anything.

## Raw mode is entered per line

The terminal is put into raw mode when a line starts and restored when it is
submitted. Holding it across the command would hand every child a terminal that
does not echo and does not turn Ctrl-C into a signal — the shell's editor
imposed on programs that never asked for it.

A `Drop` guard makes it automatic, on every path out including a panic. The same
reasoning as [redirection](redirection.md) and [terminal
modes](terminal.md): a restore that has to be remembered is a restore that will
eventually be forgotten.

## The editor does no I/O

Keys in, an action out. Rendering is a pure function from state to bytes, and
the caller does the reading and writing.

This is not tidiness for its own sake. A line editor is mostly edge cases —
word deletion at the start of a line, history navigation with a half-typed line
held back, completing a word that is not the last one — and each is a two-line
test when the editor is a state machine, or a pseudoterminal session when it is
not. The 47 unit tests in `rsh-line` run in under a millisecond; the 14 that
genuinely need a terminal take six seconds.

## Decoding keys

A keypress is one byte or six. Arrow keys, Home, End, and Delete arrive as
escape sequences — `ESC`, usually `[`, then parameters and a final letter —
with no length prefix and no framing.

This is why Escape feels laggy in terminal programs: `ESC` alone and `ESC`
starting a sequence are the same first byte, so a program must either wait or
guess.

`rsh` takes the third option. The decoder is handed a buffer and reports how
much it consumed, so an incomplete sequence stays there until more bytes arrive.
No timers, no guessing, and the ambiguity never has to be resolved.

## Redraw, don't patch

Every keystroke redraws the whole line: return to column 0, erase to end of
line, write the prompt and buffer, position the cursor.

Patching only what changed would be fewer bytes and considerably more code, and
every bug in it would look like a line that is wrong on screen while the buffer
is fine — the worst kind to diagnose. A redraw is a few dozen bytes over a link
that carries megabytes.

**What this does not handle:** a line longer than the terminal is wide. The
escape sequences address a single row, so a wrapped line leaves fragments
behind. Doing it properly needs the terminal's width *and* the display width of
each character — and character width is genuinely hard, because East Asian
characters take two cells, combining marks take none, and emoji disagree with
everyone. Phase 8 stops short, and says so rather than shipping a width table
that is subtly wrong.

## History is filtered by what you have typed

Up on an empty line walks back through history, as everywhere. Up on a
half-typed line finds the last command *starting with it*: type `git ` and Up
finds the last `git` command rather than the last command.

This is fish's behaviour rather than bash's default, and it is a deliberate
choice. The empty prefix matches everything, so the familiar case is unchanged;
the difference only appears when the user has given the shell something to work
with, and then it is almost always what they meant.

The consequence worth knowing: Up on a line matching nothing does nothing, where
bash would have jumped to the last command.

The half-typed line is held back and restored by Down. Losing it is one of the
most irritating things a shell can do.

## Completion is positional

| The word being typed | Completed as |
| --- | --- |
| first on the line | a command: builtins, then `PATH` |
| starts with `$` | an environment variable |
| contains a `/` | a path, even in first position |
| anything else | a path |

The `/` rule overrides position because a word with a slash in it is a path by
definition — the same rule `PATH` lookup itself follows, which is why
`./script` runs the script here and not something on `PATH`.

Several matches fill in as far as the candidates agree and then list them.
Completing to the common prefix is what makes a second Tab feel like progress
rather than a menu reappearing unchanged.

Directories come back with a trailing `/`, which says the completion is not
finished and lets the next Tab continue without a keystroke in between.

**Not handled:** quoting. Completing inside `"some file` treats the space as a
word boundary. The completion still lands in the right place; it just offers the
wrong candidates.

## Did you mean

A shell that only says `command not found` is telling the user something they
already know. The useful part is which of `sl`, `ls`, and `lz` they meant.

```console
rsh> grepp pattern
rsh: grepp: command not found
      did you mean `grep`?
```

Levenshtein distance against every executable on `PATH`, with a tolerance that
scales with the length of what was typed — one edit for short names, two for
longer ones. Two rules keep it from being noise:

- **Names under three characters get no suggestion at all.** Every two-letter
  command is one edit from every other.
- **A distance of zero is skipped.** That means a file of exactly that name
  exists but could not be run — a directory, or something without the execute
  bit — and suggesting the name back is pure noise.

A wrong suggestion is worse than none, because it sends the reader looking in
the wrong place.

## Configuration

`~/.rshrc`, read at startup, one command per line, each run through exactly the
same code path a typed command takes. There is no configuration language and no
second parser to disagree with the real one.

It runs only when interactive. A script that sourced the user's rc file would
behave differently depending on whose machine it ran on, which is precisely what
a script must not do.

A line that fails reports itself and the rest still run — the right behaviour
for a file whose effects the user cannot see.

## Not implemented

- **Reverse incremental search** (Ctrl-R). Prefix search covers most of what it
  is used for.
- **A kill ring.** Ctrl-U, Ctrl-K, and Ctrl-W discard rather than store, so
  there is no Ctrl-Y to paste them back.
- **Multi-line editing.** A command spanning lines cannot be typed, because the
  parser has no continuation prompt either — the two would have to arrive
  together.
- **Syntax highlighting and autosuggestions.** Both need the renderer to
  understand widths and wrapping first.
- **Programmable completion.** No per-command rules; `git ` completes to paths
  like anything else.
