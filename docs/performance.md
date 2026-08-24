# Performance

Every number here was measured on the machine this was written on — a WSL2
Linux on a desktop — with `cargo bench`. They are not claims about your
machine. The point is the **ratios**, which hold everywhere.

## The one number that matters

```text
parse a pipeline      1.3 µs
expand its words      0.3 µs
resolve a command     4.9 µs
─────────────────────────────
start one process   492.0 µs      ← 75× everything above, combined
```

A shell spends its life waiting for `fork`, `exec`, and the loader. Optimising
the parser would be polishing a rounding error: parsing `cat f | grep x | sort`
takes about a four-thousandth of the time spent starting the three processes it
describes.

This is why the benchmarks that guard against regressions are the *parsing*
ones — not because parsing is expensive, but because it is the only part whose
timing is stable enough to watch.

## Measured, `cargo bench`

**Parsing** — pure computation, no syscalls:

| | |
| --- | --- |
| `ls` | 163 ns |
| `grep -rn pattern src/` | 594 ns |
| `echo "a $HOME b" 'literal $HOME' c\ d` | 695 ns |
| `cat f \| grep rust \| sort -u \| head -20` | 1.30 µs |
| `cmd < in > out 2>&1 3>&2` | 979 ns |

**Expansion**:

| | |
| --- | --- |
| literal words | 115 ns |
| `$HOME/$USER` | 229 ns |
| `"$LIST"` (quoted, one field) | 109 ns |
| `$LIST` (split into five) | 304 ns |

**Processes**:

| | |
| --- | --- |
| `pipe()` | 1.77 µs |
| resolve `/bin/sh` (absolute) | 1.24 µs |
| resolve `sh` (searches `PATH`) | 4.90 µs |
| **fork + exec + wait `/bin/true`** | **492 µs** |
| resolve a name that does not exist | **24 ms** |

## The thing the benchmarks found

That last row is not a typo. A **failed** lookup costs five thousand times a
successful one, because success stops at the first match and failure has to
exhaust `PATH`.

Worse, Phase 8 added "did you mean", which runs *after* a failed lookup and
reads every directory on `PATH` rather than stat-ing one name in each. Measured:
**61 ms**. Nobody had ever timed it.

So a typo costs 85 ms end to end on this machine. That is perceptible.

Splitting the cause from the symptom:

```console
$ # this machine's PATH: 32 entries, 17 of them Windows directories
$ time whelk -c 'grepp x'          # full PATH
0.085s
$ time PATH=$LINUX_ONLY whelk ...  # 15 entries, 2255 executables
0.014s
```

The algorithm is fine — 14 ms to scan 2,255 executables and compute an edit
distance for each. The other 71 ms is WSL's bridge to the Windows filesystem,
where every `readdir` crosses a 9p connection.

**No change was made.** The cost is environmental, it only occurs after the user
has already made a mistake and is about to read a message, and `bash` on the
same machine pays the same enumeration cost for its own completion. Measuring it
and deciding not to act is the whole of "measure before optimizing" — the
alternative was optimising a 14 ms path on the strength of an 85 ms number.

## `whelk --benchmark`

For a figure comparable against another shell in a few seconds:

```console
$ whelk --benchmark
whelk benchmark
────────────────────────
startup          0.48 ms
echo             1.01 ms
pipeline         1.18 ms
memory            2.3 MB
```

A **release** build. The debug binary reports roughly twice these figures —
0.82 ms of startup rather than 0.48 — which is the first thing to check when a
number looks wrong.

Every row is end to end — a fresh shell process, the kernel, and where relevant
another program's startup. That is the honest unit: a user waiting for `echo hi`
waits for all of it.

The median of thirty runs, not the mean: on a shared machine there is always one
scheduling hiccup, and a mean reports it as though it were typical.

## Regression tests that are not about time

CI wall-clock varies several-fold between runs. A threshold loose enough to pass
reliably there is too loose to catch anything a person would notice; a tight one
fails on a busy afternoon and gets disabled. Timing belongs in `cargo bench`,
where a human reads the numbers.

**Allocation counts** are the opposite — deterministic, machine-independent, and
a good proxy for what actually goes wrong in a parser: a rewrite that quietly
starts copying a `String` per token.

| line | allocations | ceiling |
| --- | --- | --- |
| `ls` | 8 | 14 |
| `grep -rn pattern src/` | 22 | 34 |
| four-stage pipeline | 44 | 66 |
| heavily quoted | 26 | 40 |
| `$HOME/$USER/${PATH}` | 24 | 36 |

A counting global allocator in the test binary does the measuring. One caution
learned the hard way: the counter is **process-global**, so it counts every
allocation made anywhere while it runs — including ones made on another thread.
libtest runs each test on its own thread and formats results on the main one,
and that formatting allocates. Parsing `ls` measures its true 8 alone and
anywhere from 15 to 62 beside one other test.

Collapsing the assertions into a single test is not enough: the harness's own
threads are still there, which is how the budget first passed on Linux and
failed on macOS at 17. Filtering by thread is the obvious repair and the wrong
one — asking which thread you are on means touching thread-local storage from
inside the global allocator, and on some platforms the first such touch
allocates, straight back into the allocator that asked.

So the target sets `harness = false` and is a plain `main`: one thread, nothing
else running in it, the same number every time. It prints the table on every
run, pass or fail, because the numbers are the point — a run that only says
`ok` gives nobody the figure to write into the ceilings after a deliberate
change.

## Tracing

```console
$ WHELK_TRACE=1 whelk
whelk> echo hi | tr a-z A-Z
trace  parse             14.4µs  input=20
trace  expand             2.6µs  words=2
trace  expand             657ns  words=3
trace  resolve           20.5µs  program=echo
trace  resolve           19.5µs  program=tr
trace  spawn            226.4µs  stages=2
HI
trace  wait              19.9ms
```

Off unless `WHELK_TRACE` is set; one relaxed atomic load per call site when off.

The shell does not use the `tracing` crate, and the reason is specific to this
program: the most interesting moment in a shell is the window between `fork` and
`exec`, where **nothing may allocate**. A subscriber that formats an event into
a `String` is exactly what must not happen there. What `whelk-trace` offers
instead is a rule it can keep — spans are opened and closed in the parent, never
across a fork. Adopting `tracing` later would mean rewriting two macro bodies;
the call sites would not change.

## Recipes not run here

This machine has no `perf`, `strace`, or `valgrind`, so the following are
procedures rather than results. Saying so is the point: a document that printed
plausible flamegraph output without having produced one would be worse than
useless.

**Flamegraph**

```console
$ cargo install flamegraph
$ cargo flamegraph --bench processes -- --bench
```

The release profile sets `debug = 1`, so symbols survive without the size of
full debug info. Expect the graph to be almost entirely kernel time under
`fork` and `execve`; anything else standing out is a finding.

**Syscall counts**

```console
$ strace -c -f whelk -c 'echo hi | grep h'
```

The count that matters is per command: a shell doing thirty syscalls to run one
program is doing something structural, and `-f` is essential because everything
interesting happens in a child.

**Allocation profiling**

```console
$ valgrind --tool=dhat ./target/release/whelk -c 'echo hi'
```

The counting allocator above answers "how many"; DHAT answers "which, and how
long did they live". Reach for it when a ceiling moves and the cause is not
obvious from the diff.

## What is not measured

- **Throughput through a pipeline.** The interesting figure is bytes per second
  through `a | b`, which is dominated by the 64 KiB pipe buffer and by both
  programs — not by the shell.
- **Startup with a large history.** History is read at startup and the file is
  capped at 5,000 lines, so the cost is bounded but unmeasured.
- **Completion latency on a slow filesystem.** The `suggest` number above is a
  hint that this would be worth knowing.
