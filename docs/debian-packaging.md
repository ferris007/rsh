# Getting into Debian

`sudo apt install whelk` on a machine that has added nothing needs whelk to be
in Debian's archive. Ubuntu imports from Debian, so Debian is the only door
worth knocking on.

This is a long road with a person at the end of it, and most of the waiting is
not yours to control. What follows is where it stands and what is left.

## What is already done

The `debian/` directory is complete and builds:

| | |
| --- | --- |
| `control` | source and binary stanzas, build dependencies |
| `rules` | explicit build; see below for why not dh-cargo |
| `copyright` | DEP-5, MIT or Apache-2.0 |
| `changelog` | 0.2.1-1, with the ITP number still to fill in |
| `watch` | follows GitHub tags |
| `whelk.1` | manual page |
| `patches/` | trims the workspace to the shell |

[The workflow](../.github/workflows/debian.yml) builds it in a `debian:sid`
container with only what `control` declares, using Debian's `rustc` and
Debian's packaged crates, with no network — the constraint that makes this
different from `cargo build`. It then installs the result and runs it, because
a package that builds is not the same as a package that works.

lintian reports two warnings, both of which are the placeholder ITP number:

```
W: whelk: initial-upload-closes-no-bugs
W: whelk: wrong-bug-number-in-closes #NNNNNN
```

They go away when the bug is filed and its number goes in the changelog.

### Two decisions worth knowing about

**`nix` is pinned to 0.30, not 0.31.** Debian sid ships 0.30.1. Requiring 0.31
would mean getting a new version of someone else's crate into Debian first — a
second sponsorship, for no gain. The API this shell uses did not change between
them, and all 340 tests pass on 0.30.

**`debian/rules` does not use `dh-cargo`.** dh-cargo is built for packaging a
single crate unpacked from a crates.io tarball: it derives `cargo install
--path` from the source root and offers no way to point it elsewhere, so on a
workspace it finds a virtual manifest and refuses. This repository is a
workspace of nine crates. The rules therefore configure the source replacement
dh-cargo would have set up, and build and install in two plain steps.

## What is left, and who has to do it

### 1. File an ITP

Intent To Package, against the `wnpp` pseudo-package:

```console
$ sudo apt install reportbug
$ reportbug --email your@email wnpp
```

Choose **ITP**, package `whelk`. You will be asked for a description, the
licence (MIT or Apache-2.0), the language (Rust), and the upstream URL. You are
upstream, which is worth saying plainly in the bug.

Then put the number Debian sends you into `debian/changelog`, replacing
`#NNNNNN`, and both lintian warnings resolve.

### 2. Find a sponsor

You cannot upload to Debian yourself; a Debian Developer must review and sign
for it. This is the part with no timetable:

- Put the source package on [mentors.debian.net](https://mentors.debian.net)
- Post an RFS (Request For Sponsorship) to `debian-mentors@lists.debian.org`
- The [Debian Rust team](https://salsa.debian.org/rust-team) is the natural home
  for a Rust package, and reviews there tend to be faster than a cold RFS

Expect review comments. A sponsor may well restructure `debian/rules`, and
they are usually right.

### 3. Wait for NEW

Every genuinely new source package is reviewed by the ftpmasters, mostly for
licensing. Weeks to months.

### 4. Wait for Ubuntu

Ubuntu imports from Debian automatically, but only into the release under
development. **Nobody on 22.04 or 24.04 will ever get whelk from the archive.**
The earliest an `apt install whelk` works without adding anything is the first
Ubuntu released after Debian accepts it.

That last point is the one most worth being clear-eyed about: this route serves
future users. The [apt repository](apt-repository.md) serves the ones who exist
now.

## Building it yourself

```console
$ sudo apt install devscripts equivs lintian
$ mk-build-deps --install --remove debian/control
$ dpkg-buildpackage -us -uc -b
$ lintian --info --display-info --pedantic ../whelk_*.changes
```
