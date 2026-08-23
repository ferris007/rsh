# Parsing

How a line of text becomes a tree, and where the shell decides what things mean.

## Three steps, not one

```text
   "echo $HOME | grep -v tmp > out"
                 │
                 ▼
   ┌──────────────────────────┐
   │  lexer                   │   characters → tokens
   │  rsh-parser::lexer       │   quote removal, escapes,
   └────────────┬─────────────┘   `$NAME` recognised (not resolved)
                │
                ▼
   ┌──────────────────────────┐
   │  parser                  │   tokens → Pipeline / Command / Redirect
   │  rsh-parser::parser      │
   └────────────┬─────────────┘
                │            ── crate boundary; nothing above has touched the OS
                ▼
   ┌──────────────────────────┐
   │  expansion               │   words → argv
   │  rsh-executor::expand    │   `$HOME` resolved, fields split
   └──────────────────────────┘
```

The boundary is the interesting part. Everything above it is a pure function
from a `&str` to a tree, testable with no environment, no filesystem, and no
child process. Everything below needs to know what `HOME` is.

## Why expansion is not in the parser

The roadmap lists "environment expansion" under the parser phase, and it would
have been easy to put it there. Reading `$HOME` while scanning the word is two
lines of code.

It would also have made the parser untestable in the way that matters. A parser
that resolves variables needs a live environment to produce any output at all,
so every test of quoting or operator precedence would drag a process
environment behind it, and any test of expansion would be a test of the machine
it ran on.

Splitting it costs one extra type — [`WordPart`] — and buys two independent test
suites: the *syntax* of `"$X"` is checked with no environment, and the
*semantics* are checked against a `HashMap`.

There is also a correctness argument. In POSIX, expansion genuinely is an
execution-time step: it happens after parsing, once per execution, and its
result depends on state that earlier commands in the same line may have
changed. A shell that expanded during parsing would have to explain why
`X=1; echo $X` sees the old value. `rsh` cannot run `;` yet, but the design
should not be the reason.

## Why a word is not a string

By the time quote removal is done, `hello` is finished but `"$HOME/bin"` is not.
A `String` cannot record the two things the shell still needs to know:

- which parts came from an expansion — because only those are split into
  fields; and
- whether the expansion was quoted — because `"$X"` is always exactly one
  field, while `$X` may be several, or none.

So a word is a list of parts:

```text
"$HOME/bin"     →  [ Parameter{ HOME, quoted: true }, Literal("/bin") ]
$HOME/bin       →  [ Parameter{ HOME, quoted: false }, Literal("/bin") ]
'$HOME'         →  [ Literal("$HOME") ]
```

## Field splitting

The rule is narrower than people expect: **only the result of an unquoted
expansion is split.** Literal text is never split, which is why `echo a\ b`
stays one argument.

| Input | `X` | Arguments |
| --- | --- | --- |
| `echo $X` | `a b` | `a`, `b` |
| `echo "$X"` | `a b` | `a b` |
| `echo $X` | *unset* | *(none)* |
| `echo "$X"` | *unset* | `` (one empty) |
| `echo ""` | — | `` (one empty) |

The unset row is the one that bites: `cmd $EMPTY` passes zero arguments and
`cmd "$EMPTY"` passes one. Half of all "why did my script break on an empty
variable" bugs are that distinction.

Implementation-wise the whole rule is one `Option<String>`: `None` means no
field is open, `Some("")` means an empty field is open. A separator closes the
open field; a literal or quoted part opens one unconditionally.

## Tokens that depend on context

`2` is an argument in `echo 2` and a file descriptor in `2>err`. The difference
is whether a redirection operator follows *immediately*, with no space — which
is a question you can answer while scanning characters and cannot answer from a
token stream that has already thrown the spacing away. So the lexer decides it,
and emits either `Word` or `IoNumber`.

This is why shells lex and parse separately even though their grammars are
small. `echo 2 > f` and `echo 2> f` do different things, and only the character
scanner knows which one it saw.

## Spans, and why every node has one

Every token, word, and error carries a byte range. It costs two `usize` per
node and it buys this:

```console
rsh> echo hi > out.txt
rsh: redirection is not implemented yet (roadmap phase 3)
  echo hi > out.txt
          ^^^^^^^^^
```

`bash: syntax error near unexpected token '|'` is what a shell says when it has
thrown that information away. Spans are cheap; the alternative is a message
that cannot be improved later because the data is gone.

## What is deliberately refused

The lexer recognises far more than the shell can run. Everything it recognises
but cannot execute is reported by name, with the roadmap phase that will
deliver it:

| Syntax | Status |
| --- | --- |
| `\|` | parsed; execution is Phase 4 |
| `<`, `>`, `>>`, `<&`, `>&`, `2>` | parsed; execution is Phase 3 |
| `&` | refused — Phase 6 |
| `&&`, `\|\|`, `;` | refused — not yet on the roadmap |
| `<<` | refused — needs multi-line input |
| `$(...)`, backticks | refused — command substitution |
| `${X:-default}`, `$1` | refused — only plain `${name}` so far |
| `*`, `?`, `[...]` | passed through literally — no globbing yet |

The first two rows are the Phase 2 result: the shell now parses the whole
construct, builds the tree, and *then* declines to run it. `echo hi > out.txt`
does not create `out.txt`.

Globbing is the one case that passes through silently, and that is not an
oversight: a POSIX shell leaves a pattern unchanged when it matches nothing, so
an unexpanded `*.txt` is a behaviour a real shell has too. It is still a gap,
just not a lie.

[`WordPart`]: ../crates/rsh-parser/src/word.rs
