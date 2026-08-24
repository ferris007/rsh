# Experiments

Each directory here is a standalone program that answers **one** concrete
question about how Unix behaves, with a runnable demonstration and a recorded
observation.

They exist because the alternative — asserting in a comment that `_exit` is
required, or that a directory on `PATH` would break `exec` — is unfalsifiable.
An experiment can be run, and its conclusion is a test that fails if a platform
changes its mind.

The format is deliberately fixed:

```text
experiments/<name>/
├── README.md    the question, how to run it, the observation, what it means
├── src/         the program
└── tests/       the observation, as an assertion
```

## Index

| Experiment | Question |
| --- | --- |
| [`fork_exec`](fork_exec/) | A forked child that terminates with `exit()` instead of `_exit()` — what does it take with it? |
| [`file_descriptors`](file_descriptors/) | `dup2` clears close-on-exec on its target. What happens when source and target are the same descriptor? |
| [`pipes`](pipes/) | A Rust program ignores `SIGPIPE` before `main` runs. What happens to the programs it `exec`s? |
| [`signals`](signals/) | You press Ctrl-C. Who actually gets the signal? |
| [`process_groups`](process_groups/) | Two processes, same terminal, both blocked in `read`. What happens to the one that does not own it? |
| [`pty`](pty/) | A program prints progress as it works. Pipe it and the progress stops appearing. Where did it go? |
| [`epoll`](epoll/) | A signal handler sets a flag and the loop checks the flag. What is missing? |
| [`namespaces`](namespaces/) | You call `unshare(CLONE_NEWPID)`. What is your process id now? |

More arrive alongside the phases that need them; see
[the roadmap](../docs/roadmap.md).
