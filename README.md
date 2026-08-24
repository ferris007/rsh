<h1 align="center">rsh</h1>

<p align="center">
  <strong>A Unix shell built from first principles in Rust.</strong><br>
  Processes, file descriptors, pipelines, signals, job control, terminals, and an event loop — implemented, measured, and explained.
</p>

<p align="center">
  <a href="https://github.com/ferris007/rsh/actions/workflows/ci.yml"><img src="https://github.com/ferris007/rsh/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/rust-1.85%2B-orange.svg" alt="Rust 1.85+">
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg" alt="MIT OR Apache-2.0">
  <img src="https://img.shields.io/badge/tests-334-brightgreen.svg" alt="334 tests">
  <img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20BSD-lightgrey.svg" alt="Linux, macOS, BSD">
</p>

---

```console
$ rsh
rsh> printf '3\n1\n2\n' | sort | head -2 > out.txt
rsh> cat < out.txt
1
2
rsh> sleep 30 &
[1] 4242
rsh> sleep 30
^Z
[2]+  Stopped                 sleep 30
rsh> jobs
[1]-  Running                 sleep 30
[2]+  Stopped                 sleep 30
rsh> grepp pattern
rsh: grepp: command not found
      did you mean `grep`?
rsh> exit
rsh: there are stopped jobs
rsh> exit
```

## What this is

`rsh` is a **systems-engineering project**, not a Bash replacement. Its purpose
is to make the operating system's process model visible instead of hiding it:
every feature is built on primitives that were understood first, and the
reasoning is written down beside the code.

The goal is a shell **small enough to understand and deep enough to expose how
Unix actually works**.

> **Scope.** `rsh` runs real command lines and is pleasant to use, but it is not
> a drop-in shell. It has no `&&`, `||`, `;`, globbing, command substitution, or
> multi-line input. See [Not implemented](#not-implemented) — the list is
> deliberate and complete.

## Features

| | |
| --- | --- |
| **Parsing** | Lexer, AST, quoting, escaping, errors that point at the offending column |
| **Expansion** | `$VAR`, `${VAR}`, `$?`, `$$`, `~`, `IFS` field splitting |
| **Redirection** | `>`, `>>`, `<`, `2>`, `2>&1`, arbitrary descriptors, for builtins too |
| **Pipelines** | Concurrent processes, correct exit status, broken-pipe handling |
| **Signals** | `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGCHLD`, `SIGWINCH`, graceful shutdown |
| **Job control** | `jobs`, `fg`, `bg`, Ctrl-Z, process groups, terminal ownership |
| **Terminal** | Mode save/restore, resize, raw mode — `vim` and `top` work inside it |
| **Line editing** | History with prefix search, Tab completion, `~/.rshrc` |
| **Event loop** | `epoll` on Linux, `kqueue` on BSD and macOS |
| **Observability** | Criterion benchmarks, `RSH_TRACE`, allocation budgets |

## Install

Requires **Rust 1.85** or newer. No system dependencies beyond a Unix kernel.

```console
$ git clone https://github.com/ferris007/rsh
$ cd rsh
$ cargo build --release
$ ./target/release/rsh
```

Or run it straight from the workspace:

```console
$ cargo run --release -p rsh
```

## Usage

### Builtins

| Command | |
| --- | --- |
| `cd [dir\|-\|~]` | Change directory; `-` returns to the previous one |
| `exit [status]` | Leave, defaulting to the last command's status |
| `jobs` | List jobs with their state |
| `fg [%n]` | Resume a job in the foreground |
| `bg [%n]` | Resume a job in the background |

Job specifiers accept `%1`, `%%`, `%+`, `%-`, or a bare `1`. With no argument,
the current job.

### Key bindings

| Key | | Key | |
| --- | --- | --- | --- |
| `←` `→` | Move by character | `Ctrl-A` / `Home` | Start of line |
| `↑` `↓` | History, filtered by what you typed | `Ctrl-E` / `End` | End of line |
| `Tab` | Complete command, path, or `$VAR` | `Ctrl-U` | Delete to start |
| `Ctrl-C` | Abandon the line | `Ctrl-K` | Delete to end |
| `Ctrl-D` | End of input, or delete forward | `Ctrl-W` | Delete previous word |
| `Ctrl-Z` | Suspend the running job | `Ctrl-L` | Clear the screen |

Typing `git ` and pressing `↑` finds the last `git` command rather than the
last command — the empty prefix matches everything, so the familiar case is
unchanged.

### Files and environment

| | |
| --- | --- |
| `~/.rshrc` | Run at startup, one command per line, interactive sessions only |
| `~/.rsh_history` | Persisted history, capped at 5,000 entries |
| `RSH_TRACE=1` | Print timed, structured diagnostics to stderr |
| `COLUMNS`, `LINES` | Set by the shell and kept current on resize |

### Command line

```console
$ rsh                    # interactive
$ rsh < script.sh        # non-interactive; no prompt, no job control
$ rsh --benchmark        # measure this machine
```

## Performance

Measured with `cargo bench`. The ratios are the point:

```text
parse a pipeline      1.3 µs
expand its words      0.3 µs
resolve a command     4.9 µs
─────────────────────────────
start one process   492.0 µs      ← 75× everything above, combined
```

A shell spends its life waiting for `fork` and `exec`. Optimising the parser
would be polishing a rounding error — which is why the regression tests count
**allocations** rather than time: CI wall-clock varies several-fold, and a
threshold loose enough to pass reliably is too loose to catch anything.

```console
$ rsh --benchmark
rsh benchmark
────────────────────────
startup          0.48 ms
echo             1.01 ms
pipeline         1.18 ms
memory            2.3 MB
```

A release build; a debug one is roughly twice as slow, which is worth knowing
before comparing these against another shell.

Full numbers, and the 61 ms cost the benchmarks found in a feature nobody had
timed, are in [docs/performance.md](docs/performance.md).

## Architecture

```text
   user input →  rsh              REPL, prompt, line editing
                  │
                  ▼
                 rsh-parser       text → AST; never touches the OS
                  │
                  ▼
                 rsh-executor     expansion, builtins, dispatch, shell state
                  │
                  ▼
                 rsh-process      fork / exec / wait, pipes, signals — all the `unsafe`
                  │
                  ▼
                 kernel

   alongside:    rsh-job          the job table and process groups
                 rsh-terminal     modes, size, ownership
                 rsh-line         editing, history, completion
                 rsh-event        readiness over epoll and kqueue
                 rsh-trace        timed diagnostics
```

The split is not decoration. `rsh-parser` turns a string into a tree and can be
tested exhaustively without spawning anything; `rsh-process` holds every
`unsafe` block that touches the process table, so the argument for the
fork/exec window fits in one file.

See [docs/architecture.md](docs/architecture.md).

## Experiments

Each directory in [`experiments/`](experiments/) answers **one** concrete
question with a runnable program and an observation pinned as a test.

| Experiment | Question |
| --- | --- |
| [`fork_exec`](experiments/fork_exec/) | A child terminates with `exit()` instead of `_exit()`. What does it take with it? |
| [`file_descriptors`](experiments/file_descriptors/) | `dup2` clears close-on-exec. What if source and target are the same? |
| [`pipes`](experiments/pipes/) | Rust ignores `SIGPIPE` before `main`. What happens to the programs it `exec`s? |
| [`signals`](experiments/signals/) | You press Ctrl-C. Who actually gets the signal? |
| [`process_groups`](experiments/process_groups/) | Two processes, one terminal, both blocked in `read`. What happens to the one that does not own it? |
| [`pty`](experiments/pty/) | A program prints progress. Pipe it and the progress stops. Where did it go? |
| [`epoll`](experiments/epoll/) | A handler sets a flag and the loop checks the flag. What is missing? |
| [`namespaces`](experiments/namespaces/) | You call `unshare(CLONE_NEWPID)`. What is your process id now? |

Seven of the eight came out of a bug found while building the phase they
document. That was not the plan; it is what happened every time.

## Documentation

| | |
| --- | --- |
| [Architecture](docs/architecture.md) | How the crates split, and why |
| [Parsing](docs/parsing.md) | Text to tree, and where meaning gets decided |
| [Process model](docs/process-model.md) | `fork`, `exec`, and the window between |
| [Redirection](docs/redirection.md) | `dup2`, ordering, and close-on-exec |
| [Pipelines](docs/pipelines.md) | Concurrency, backpressure, and broken pipes |
| [Signals](docs/signals.md) | What a handler may do, and who receives Ctrl-C |
| [Job control](docs/job-control.md) | Process groups and the foreground slot |
| [Terminal](docs/terminal.md) | Modes, size, and state that outlives a process |
| [Line editing](docs/line-editing.md) | Raw mode, history, and completion |
| [Events](docs/events.md) | `epoll`, `kqueue`, and waiting for several things |
| [Performance](docs/performance.md) | Measured numbers, and one they found |
| [Roadmap](docs/roadmap.md) | All eleven phases, and what is deliberately unfinished |

## Testing

```console
$ cargo test --workspace       # 334 tests
$ cargo bench                  # criterion benchmarks
$ cargo clippy --workspace --all-targets -- -D warnings
```

Twenty-nine of those tests drive the shell through a real **pseudoterminal**.
Job control and terminal handling cannot be observed any other way: Ctrl-Z only
exists because a terminal driver turns a keystroke into a signal for the
foreground process group, and a shell reading from a pipe has none of that.

## Not implemented

Deliberate, and complete. Syntax in the first group is **parsed and refused by
name** — never silently treated as an argument.

| | |
| --- | --- |
| `&&`, `\|\|`, `;` | Need a list grammar above the pipeline |
| Here-documents (`<<`) | Needs multi-line input, which the editor lacks too |
| Command substitution | `$(...)` and backticks |
| `${X:-default}`, `$1` | Only plain `${name}` is supported |
| Subshells `( )` | Also what builtins in a pipeline would need |
| Globbing | `*.txt` passes through unchanged, as a POSIX shell does when nothing matches |
| `set -e`, `-u`, `pipefail` | Each changes the meaning of code already written |
| Long-line redraw | The editor draws one row; doing better needs character widths |

## Principles

- **Systems first** — understand OS primitives instead of hiding them behind
  abstractions.
- **Safe by default** — lean on the type system; isolate `unsafe` to the places
  where the *semantics* are genuinely unsafe.
- **Observable** — behaviour is exposed through tests and diagnostics, not
  asserted in prose.
- **Incremental complexity** — every feature builds on a previously understood
  primitive.
- **Portable where practical** — Unix-like systems, Linux first.
- **Performance-aware** — measure before optimizing, and be willing to conclude
  *don't*.

## Contributing

Bug reports, corrections, portability fixes, and new experiments are all
welcome. One unusual rule, stated up front:

> Please do not send a pull request that implements a future roadmap phase.

See [CONTRIBUTING.md](CONTRIBUTING.md) for why, and for what the project
genuinely needs.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
