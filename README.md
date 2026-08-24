# rsh

[![CI](https://github.com/ferris007/rsh/actions/workflows/ci.yml/badge.svg)](https://github.com/ferris007/rsh/actions/workflows/ci.yml)

A Unix shell built from first principles in Rust — exploring processes, file
descriptors, pipelines, signals, job control, terminals, and event-driven
systems.

`rsh` is not a Bash replacement. It is a systems-engineering project: an
experimental shell whose purpose is to make the operating system's process
model visible instead of hiding it. Every feature is implemented on top of
primitives that were understood first.

The goal is a shell **small enough to understand and deep enough to expose how
Unix actually works**.

## Status

Phase 7 — terminal management. The process model is complete: `vim`, `top`, and
`less` work inside `rsh`, and a job that wrecks the terminal cannot leave it
that way.

```console
$ cargo run -p rsh
rsh> stty -echo          # a job turns off echoing and exits
rsh> echo still visible  # ...and the shell has already put it back
still visible
rsh> sleep 30 &
[1] 4242
rsh> sleep 30
^Z
[2]+  Stopped                 sleep 30
rsh> jobs
[1]-  Running                 sleep 30
[2]+  Stopped                 sleep 30
rsh> echo terminal is ${COLUMNS}x${LINES}
terminal is 120x30
rsh> exit
rsh: there are stopped jobs
rsh> exit
```

`dash` fails that first example: after `stty -echo` it leaves the terminal
silent and you type blind. Terminal state outlives the process that changed it,
and the shell is the only thing still running that knows what it was before.

The interactive layer, the event-driven execution model, and the benchmark suite
are still ahead. Syntax the shell cannot run yet is parsed, refused by name, and
pointed at — never silently treated as an argument.

See [`docs/roadmap.md`](docs/roadmap.md) for the full plan and what lands when.

## Principles

- **Systems first** — understand OS primitives instead of hiding them behind
  abstractions.
- **Safe by default** — lean on Rust's type system; isolate `unsafe` to the
  handful of places where the *semantics* are genuinely unsafe.
- **Observable** — process, pipeline, signal, and performance behaviour is
  exposed through tests and diagnostics, not asserted in prose.
- **Incremental complexity** — every feature builds on a previously understood
  primitive.
- **Portable where practical** — Unix-like systems, with Linux as the primary
  development platform.
- **Performance-aware** — measure before optimizing.

## Building

```console
$ cargo build
$ cargo test
$ cargo run -p rsh
```

Requires Rust 1.85 or newer (see `rust-version` in `Cargo.toml`).

## Repository layout

```text
crates/rsh            the REPL: read a line, dispatch it, loop
crates/rsh-parser     text → AST; never touches the operating system
crates/rsh-executor   expansion, builtins, dispatch, shell state
crates/rsh-job        the job table and process-group bookkeeping
crates/rsh-terminal   terminal modes, size, and ownership
crates/rsh-process    fork / exec / wait, PATH resolution — all the `unsafe`
docs/                 architecture and systems notes
experiments/          standalone programs, each answering one systems question
```

The split is not decoration. `rsh-parser` produces structure from a string and
can be tested exhaustively without spawning anything; `rsh-process` holds every
`unsafe` block that touches the process table, so the argument for the
fork/exec window fits in one file. See [Architecture](docs/architecture.md).

`experiments/` is a deliberate part of the project rather than a scratch
directory: each entry poses a single concrete question about Unix behaviour,
answers it with a runnable program, and records the observation. The notes are
part of the deliverable.

## Documentation

- [Architecture](docs/architecture.md) — how the crates split, and why
- [Process model](docs/process-model.md) — `fork`, `exec`, and the window between
- [Roadmap](docs/roadmap.md) — the phases, and what each one teaches
- [Experiments](experiments/) — one question, one program, one observation
- [Contributing](CONTRIBUTING.md) — what kind of help fits a project like this

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
