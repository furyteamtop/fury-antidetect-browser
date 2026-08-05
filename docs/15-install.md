# Installing Fury

> **There are no releases yet.** Every download link on this page points at a
> Releases page that is empty, and it will stay empty until a build has been
> signed and measured. Until then the way in is to build it: the application and
> the agent take minutes (`cargo build --release`), the browser core takes about
> three hours ([docs/03](03-chromium-fork.md)).
>
> The page is written anyway, and written first, because the instructions are
> the specification — what a release has to produce is exactly what this page
> already promises.

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
  /Users/you/Library/Application Support/Fury/core.bundle/Fury.app/Contents/MacOS/Fury
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

**It is not signed or notarised yet**, so macOS will object, and the objection
is expected rather than a sign that anything is wrong. There is no Apple
Developer certificate for this project; `tools/release/sign-core.sh` is written
and waiting for one.

**"Fury cannot be opened because the developer cannot be verified."** This is
the normal result for an unsigned application, and it is what you should expect
for the *correct* file — an earlier version of this page told you to download it
again, which was a loop with no exit. Instead:

  1. Right-click (or Control-click) Fury.app and choose **Open**, then **Open**
     again in the dialog. macOS remembers the choice for that copy.
  2. If that does not work, strip the quarantine flag:

     ```bash
     xattr -dr com.apple.quarantine /Applications/Fury.app
     ```

The same applies to the core: `fury-agent install-core` removes the flag from
what it unpacks, which is why the core needs no step of its own here.

**"Fury is damaged and should be moved to the Bin."** This one is almost never a
damaged file. It means the bundle's signature no longer matches its contents —
most often because something was added to or removed from inside `Fury.app`.
Delete it and unpack the download again.

**A core you built yourself will not start**, with a message about *Team IDs*.
That is a signing arrangement rather than a broken build; see
[tools/release/sign-core.sh](../tools/release/sign-core.sh), which explains it
in full.

## Windows

Not yet, and still not "coming soon" — but the sentence is shorter than it was.

The launcher is done: the agent and the desktop shell are ported, and the parts
that differ per operating system (named pipes instead of a Unix socket, an
explicit DACL instead of file modes, an inherited HANDLE instead of a file
descriptor, WM_CLOSE instead of SIGTERM) are compiled for
`x86_64-pc-windows-msvc` on every commit. The core patches read their config
from that HANDLE.

What is missing is one thing: nobody has run the Chromium build on a Windows
machine. So there is no Windows core, nothing has been measured on Windows, and
until a build exists this page will not offer a download. That is the same rule
the rest of this repository follows — an unmeasured release is a claim.

If you want to try the launcher against a core you built yourself,
`tools/verify-windows.ps1` checks the parts a compiler cannot: whether the
pipe's DACL actually refuses other accounts, whether the config handle survives
process creation, whether the browser closes cleanly instead of being killed.

## Linux

Not a target. This is a decision rather than a gap, and it is worth stating
plainly because an earlier version of this page promised a Linux release.

The Rust still compiles on Linux — CI runs there and contributors can run the
test suite — but there is no packaged release, no build configuration for the
core, and no plan for one. Two platforms that get tested are worth more than
three where one is a guess.

## Where things are kept

Everything Fury owns is under one directory, so "back it up" and "remove it" are
each one operation:

```
~/Library/Application Support/Fury/
  fury.db          profiles, proxies, personas, projects
  profiles/        each profile's browser data — cookies, tabs, bookmarks
  core.bundle/     the installed browser (a package, so search
                   shows one Fury rather than two)
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
