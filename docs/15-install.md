# Installing Fury

For a person who has not built anything. If you want to build from source, that
is [docs/03](03-chromium-fork.md) and the README; this page assumes you have a
computer and a browser and nothing else.

Fury is **two downloads**, and that is worth understanding before you start,
because otherwise the second one looks like a mistake:

| | what it is | size |
|---|---|---|
| **Fury** | the application: profiles, proxies, personas, teams | ~12 MB |
| **Fury core** | the browser itself — a Chromium fork | ~134 MB |

They are apart because they move apart. The application changes weekly; the core
changes when Chromium does, every six weeks or so. Bundling them would mean a
134 MB download every time a button moved. It also keeps the application's code
signature intact — a core written into a signed bundle breaks its seal, after
which macOS says the application is *damaged*, which sends you looking for a
corrupt download rather than at us.

## macOS

### 1. The application

Download `fury-<version>-macos-<arch>.tar.xz` from
[Releases](https://github.com/furyteamtop/fury/releases). `arch` is `arm64` for
any Apple-silicon Mac (M1 and later) and `x86_64` for an Intel one — if you are
not sure,  → About This Mac says which.

Unpack it and drag **Fury.app** into `/Applications`.

### 2. The browser core

Download `fury-core-<version>-macos-<arch>.tar.xz` from the same page. Then, in
Terminal:

```bash
/Applications/Fury.app/Contents/MacOS/fury-agent install-core ~/Downloads/fury-core-*.tar.xz
```

This is the one command that needs a terminal, and it does three things by hand
that are easy to get wrong: it unpacks the bundle preserving the symlinks a
macOS framework is built from, it removes the quarantine flag your browser
attached to the download, and — the step that matters — it **runs the browser
once and checks it starts**. If it prints a version, it is installed:

```
installed Fury 150.0.7871.187
  /Users/you/Library/Application Support/Fury/core/Fury.app/Contents/MacOS/Fury
```

Ask it at any time what is installed:

```bash
/Applications/Fury.app/Contents/MacOS/fury-agent install-core
```

### 3. Open it

Open Fury from Applications. There is no account, no sign-up and no server: it
starts working immediately, and everything stays on this machine.

Team features — shared profiles, a shared proxy pool, per-project access — are
in **Settings → Team**, which is also where you create an account and where the
instructions for running your own server are. That is a decision you can make
later, or never.

### Verifying what you downloaded

Every release has a `SHA256SUMS` file. Download it next to the archives and:

```bash
shasum -a 256 -c SHA256SUMS
```

Two `OK` lines mean the files are what was published.

### If macOS refuses to open it

The application is signed and notarised, so it should just open. If it does not:

**"Fury cannot be opened because the developer cannot be verified."** The
download did not carry its notarisation ticket, which usually means it came
from somewhere other than the Releases page. Download it again from there.

**"Fury is damaged and should be moved to the Bin."** This one is almost never a
damaged file. It means the bundle's signature no longer matches its contents —
most often because something was added to or removed from inside `Fury.app`.
Delete it and unpack the download again.

**A core you built yourself will not start**, with a message about *Team IDs*.
That is a signing arrangement rather than a broken build; see
[tools/release/sign-core.sh](../tools/release/sign-core.sh), which explains it
in full.

## Linux

No packaged release yet. The application and the agent build in minutes:

```bash
cargo build --release
cd desktop && npm install && npm run app:build
```

The core has to be built from source ([docs/03](03-chromium-fork.md)), which
takes about six hours the first time. A Linux core release will come; it is not
a technical obstacle, only a machine that has to run the build.

## Windows

Not yet. Say so plainly rather than "coming soon": the patches are
platform-independent and the agent and shell are portable, but nobody has
produced or tested a Windows core, and shipping one that has not been measured
would be exactly the thing this project refuses to do.

## Where things are kept

Everything Fury owns is under one directory, so "back it up" and "remove it" are
each one operation:

```
~/Library/Application Support/Fury/
  fury.db          profiles, proxies, personas, projects
  profiles/        each profile's browser data — cookies, tabs, bookmarks
  core/            the installed browser
```

Set `FURY_HOME` to put it somewhere else — an external disk, or an encrypted
volume.

To remove Fury completely: delete that directory and `/Applications/Fury.app`.
Nothing is written anywhere else, and nothing is left behind on any server you
did not set up yourself.

## Updating

Download and unpack the new application over the old one. For a new core, run
`install-core` again with the new archive — it verifies the new one starts
before it replaces the one you have, so a bad download leaves you with a working
browser rather than none.

## Automatic updates

There are none, and that is a deliberate gap rather than an unfinished feature.
An updater is a channel that reaches into an anti-detect browser from outside,
on a schedule, from an address that is not the profile's proxy. Building one
that does not weaken the thing it updates needs more care than it has been given
so far, and until then the honest position is that you update when you choose
to. [docs/09](09-roadmap.md) has what a good one would have to do.
