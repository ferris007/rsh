# `namespaces` — you call `unshare(CLONE_NEWPID)`. What is your process id now?

> **Question:** a process asks for a new PID namespace. Does it become PID 1?

The obvious answer is yes, and it is wrong.

## Running it

```console
$ cargo run -p xp-namespaces
before unshare, this process is pid 1351801
after unshare, this process is pid 1351801

and the parent sees that same child as pid 1352374
the child, however, is pid 1
  — which makes it init for the namespace
```

## Observation

`unshare(CLONE_NEWPID)` **does not move the caller**. Its pid is unchanged
afterwards.

What it changes is where the caller's *children* are born. The first one is pid
1 in the new namespace — and the parent, still outside, sees it as 1352374.

The same process has two numbers at the same time, and neither is more real
than the other.

## Why it works this way

A process's pid is not a name it carries. It is an entry in a table, and a PID
namespace is a new table. Moving an existing process into one would mean
changing its pid — which is visible to everything already holding it: its
parent's `waitpid`, its process group, anything that recorded it in a file.

So the kernel does not move anyone. It changes what the *next* `fork` writes
into, and the caller stays where it is with the number everyone already knows.

```text
        outside                        inside the new namespace
   ┌──────────────────┐            ┌──────────────────────────┐
   │ 1351801  caller  │───unshare──┤ (caller is not here)     │
   │ 1352374  child   │◄──────────►│ 1  child (init)          │
   └──────────────────┘            └──────────────────────────┘
              the same process, two numbers
```

Being pid 1 is not honorary. The kernel treats it as init: it reaps orphans, it
does not get default signal actions for signals it has no handler for, and when
it exits every other process in the namespace is killed.

## The connection to a shell

Everything else in this directory answers a question `rsh` had to answer to
work. This one does not — `rsh` creates no namespaces, and a shell has no
reason to.

It is here because it is the sharpest illustration of something the rest of the
project keeps running into: **a process id is a fact about a table, not about a
process.** The same idea, in weaker forms, is behind three earlier findings:

- [`process_groups`](../process_groups/) — a signal goes to a *group*, and
  membership is what decides who dies. The pid is not the addressable thing.
- [`file_descriptors`](../file_descriptors/) — a descriptor number is an index,
  and `dup2` writes at an index. The number is not the file.
- `Child::wait` consuming `self` in `rsh-process` — a pid is meaningful only
  until it is reaped, after which the kernel may hand the number to someone
  else.

Containers are what happens when that observation is taken seriously for every
table at once: pids, mounts, network interfaces, users, hostnames.

## What this experiment does not do

It creates a user namespace as well, because that is the only kind an ordinary
user may create, and being root inside it is what makes the PID namespace
permitted. Without writing `uid_map` the child has no valid user id — which is
fine for asking about pids and not fine for much else.

A real container would also unshare the mount namespace and remount `/proc`, so
that `ps` inside reports the new table rather than the old one. Until that
happens, `/proc` still shows the outside view, which is why a half-built
container looks so convincing and behaves so strangely.

## Going further

- Add `CLONE_NEWNS`, then `mount -t proc proc /proc`, and watch `ps` inside
  report two processes instead of four hundred.
- Kill the pid-1 child and watch every other process in the namespace go with
  it.
- Read `/proc/self/ns/pid` before and after. It is a symlink to an inode number,
  and comparing two of them is how you tell whether two processes share a
  namespace.
- Try `unshare(CLONE_NEWUSER)` alone and look at `/proc/self/uid_map`. Almost
  everything a container does with users is that one file.
