# rsh

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

Phase 2 — the shell parses a real grammar. Words, quoting, escaping, and
parameter expansion (`$HOME`, `${NAME}`, `$?`, `$$`, `~`) work; pipelines and
redirections are parsed into a full syntax tree, though running them is Phase 3
and Phase 4.

```console
$ cargo run -p rsh
rsh> echo hello
hello
rsh> sh -c 'exit 7'
rsh> echo "the last command exited $?"
the last command exited 7
rsh> echo ~/src
/home/ferris/src
rsh> nope
rsh: nope: command not found
rsh> echo hi > out.txt
rsh: redirection is not implemented yet (roadmap phase 3)
  echo hi > out.txt
          ^^^^^^^^^
rsh> exit
```

That last one is the shape of the whole project: the line is lexed, parsed, and
turned into a tree with a redirection node in it — and then declined, by name,
with the phase that will deliver it. `out.txt` is not created. A shell that
quietly handed `>` to `echo` as an argument would be lying about what it does.

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
crates/rsh-executor   builtins, dispatch, shell state
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

- [Architecture](docs/architecture.md)
- [Roadmap](docs/roadmap.md)

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
