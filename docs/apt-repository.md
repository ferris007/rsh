# The apt repository

`whelk` publishes its own apt repository to GitHub Pages, so that installing it
is the command people expect:

```console
$ sudo apt install whelk
```

This is **not** the Debian archive. It is this project's repository, which is
why users have to add it first — see [Install](#install-for-users) below. Being
in Debian proper is a different and much longer road, written up in
[debian-packaging.md](debian-packaging.md).

## How it works

The [release workflow](../.github/workflows/release.yml) builds the `.deb` and
attaches it to the GitHub release. The [apt workflow](../.github/workflows/apt.yml)
then indexes and signs it. Nothing is compiled twice: the package that gets
indexed is the same file that was built, stripped, smoke-tested, and uploaded,
because an index that disagrees with the binaries it points at is the one
failure an apt repository must not have.

The layout is the ordinary one:

```
key.gpg                                   the public half, for users
pool/main/w/whelk/whelk_0.2.0-1_amd64.deb the packages themselves
dists/stable/Release                      what is here, and its checksums
dists/stable/InRelease                    the same, signed in place
dists/stable/Release.gpg                  the same, signed detached
dists/stable/main/binary-amd64/Packages   the index
```

Every release's `.deb` is collected, not only the newest, so someone pinned to
an older version can still install it.

## Setting it up

Two things are needed once, and both need your hands rather than CI's.

### 1. A signing key

An unsigned repository forces users to write `[trusted=yes]`, which turns off
the check that stops someone serving them a different `whelk`. So:

```console
$ gpg --quick-generate-key "whelk repository <your@email>" rsa4096 sign never
$ gpg --list-secret-keys --keyid-format=long
$ gpg --armor --export-secret-keys YOUR_KEY_ID > /tmp/whelk-signing.asc
```

Add two repository secrets under **Settings → Secrets and variables → Actions**:

| Secret | Value |
| --- | --- |
| `APT_GPG_PRIVATE_KEY` | the whole contents of `/tmp/whelk-signing.asc` |
| `APT_GPG_PASSPHRASE` | the passphrase, or empty if the key has none |

Then delete the exported file: `shred -u /tmp/whelk-signing.asc`. The private
key never appears in the repository, and the workflow only ever imports it into
a runner that is destroyed afterwards.

Keep a backup of that key somewhere safe. Losing it means every user has to
re-add a new one by hand.

### 2. GitHub Pages

**Settings → Pages → Build and deployment → Source: GitHub Actions.** The
workflow deploys there itself; there is no `gh-pages` branch to manage.

## Publishing

Automatic on every published release. To rebuild by hand — after adding the key
for the first time, say:

```console
$ gh workflow run apt.yml
```

## Install, for users

```console
$ curl -fsSL https://ferris007.github.io/whelk/key.gpg \
    | sudo gpg --dearmor -o /etc/apt/keyrings/whelk.gpg

$ echo "deb [signed-by=/etc/apt/keyrings/whelk.gpg] https://ferris007.github.io/whelk stable main" \
    | sudo tee /etc/apt/sources.list.d/whelk.list

$ sudo apt update && sudo apt install whelk
```

The package is statically linked and declares no dependencies, so it installs
on any Debian or Ubuntu whatever its glibc version — including the ones whose
`rustc` is far too old to build it.
