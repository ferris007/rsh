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

Phase 10 — benchmarks, tracing, and allocation-counting regression tests. The
roadmap is complete apart from two experiments.

```console
$ rsh --benchmark
rsh benchmark
────────────────────────
startup          0.82 ms
echo             1.40 ms
pipeline         1.48 ms
memory            2.8 MB
```

The number worth knowing:

```text
parse a pipeline      1.3 µs
start one process   492.0 µs      ← 75× the parse, expand, and lookup combined
```

A shell spends its life waiting for `fork` and `exec`. Optimising the parser
would be polishing a rounding error — which is why the benchmarks that guard
against regressions are the parsing ones, not because parsing is expensive but
because it is the only part whose timing is stable enough to watch.

The benchmarks also found something nobody had timed: "did you mean", added in
Phase 8, costs 61 ms. Measured, traced to WSL's Windows filesystem bridge rather
than to the algorithm, and left alone. See
[Performance](docs/performance.md).

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
crates/rsh-line       line editing, history, and completion
crates/rsh-event      readiness notification over epoll and kqueue
crates/rsh-trace      timed, structured diagnostics
benches/              criterion benchmarks
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
