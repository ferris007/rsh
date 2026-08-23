# Contributing to rsh

Thank you for looking. This project welcomes help, but it is an unusual project,
so please read this first — it will save us both time.

## What this project is

`rsh` is a learning project. The point is to understand how Unix works by
building a shell on top of primitives that were understood first.

That has one consequence that surprises people:

> **Please do not send a pull request that implements a future roadmap phase.**

If you send a working implementation of pipelines or job control, I cannot merge
it. Not because it is bad work — but because writing that code *is the project*.
Merging it would be like buying a finished model kit and calling it built.

This is the opposite of how most open-source projects work. I know. It is the
one rule that matters here.

## What is very welcome

**Corrections.** If a comment or a document says something wrong about Unix, I
want to know. This project makes many claims about `fork`, `exec`, signals, and
terminals. Some of them are probably wrong. Being wrong in public is fine;
staying wrong is not.

**Bugs.** A command that behaves differently from `bash` or `dash`, a crash, a
terminal left in a broken state. Please include:

- what you ran,
- what happened,
- what you expected,
- your OS and `rustc --version`.

**Portability fixes.** Linux is the main platform. macOS is tested in CI. If
something breaks on BSD or another Unix, a fix is welcome.

**Experiments.** New entries in [`experiments/`](experiments/) are the easiest
way to contribute something substantial. See the format below.

**Questions.** If a document did not explain something clearly, that is a bug in
the document. Open an issue.

## Experiments

Each experiment answers **one** concrete question about Unix behaviour.

```text
experiments/<name>/
├── README.md    the question, how to run it, the observation, what it means
├── src/         the program
└── tests/       the observation, written as an assertion
```

The test matters. An experiment whose conclusion lives only in prose rots the
first time a platform changes its mind. A test fails loudly instead.

Keep the question narrow. "What happens to a pipeline when the shell receives
SIGINT?" is one experiment. "How do signals work?" is not.

## Setting up

You need Rust 1.85 or newer.

```console
$ git clone https://github.com/ferris007/rsh
$ cd rsh
$ cargo build
$ cargo test
$ cargo run -p rsh
```

## Before you open a pull request

All three must pass. CI runs the same checks on Linux and macOS.

```console
$ cargo fmt --all
$ cargo clippy --workspace --all-targets -- -D warnings
$ cargo test --workspace
```

## House style

A few rules that the code follows and that review will ask about:

- **`rsh-parser` never touches the operating system.** It turns a string into an
  AST. If it needs to read a file or an environment variable, something is in
  the wrong crate.
- **`rsh-executor` never re-reads the source text.** It receives structure. If
  it needs to look at characters, the AST is missing something.
- **Every `unsafe` block has a `// SAFETY:` comment** explaining why it is
  sound. Clippy enforces the comment; review checks whether it is true.
- **Tests observe the shell from outside.** End-to-end tests spawn the real
  binary and feed it bytes. There are no test-only hooks into the internals — if
  a behaviour cannot be seen from outside the process, it is not a behaviour we
  claim to have.
- **Comments explain *why*, not *what*.** The code already says what it does.

## Commit messages

A short summary line, then a blank line, then the reasoning. Explain why the
change is right, not what the diff shows — the diff is already in the commit.

## Code of conduct

Be kind and assume good faith. That is the whole policy. Behaviour that makes
this an unpleasant place to be is not welcome, and I will act on it.

## License

By contributing, you agree that your work is licensed under the same terms as
the project: MIT or Apache-2.0, at the user's option.
