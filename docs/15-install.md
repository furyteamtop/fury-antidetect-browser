# Installing Fury

> **There are releases; macOS ones are signed, Windows ones are not.** The
> [Releases](https://github.com/furyteamtop/fury-antidetect-browser/releases)
> page has had builds since 18.08.2026; the current one is 0.1.8, for macOS on
> Apple Silicon and for Windows x64, marked pre-release. Each ships with
> `SHA256SUMS` and a `REPORT.md` — the measurement the core passed before it was
> published. Since 21.09.2026 the macOS application, core and disk image are
> signed with a Developer ID and notarised, so they open like any other
> download. The Windows installer is not signed and shows a SmartScreen
> warning, covered below. To build instead, the application and the agent take minutes
> (`cargo build --release`) and the core about three hours
> ([docs/03](03-chromium-fork.md)).
>
> This page was written before the first release existed, because the
> instructions are the specification — what a release has to produce is exactly
> what this page promises. It now describes what the releases do.

For a person who has not built anything. If you want to build from source, that
is [docs/03](03-chromium-fork.md) and the README; this page assumes you have a
computer and a browser and nothing else.

Fury is **two downloads**, and that is worth understanding before you start,
because otherwise the second one looks like a mistake:

| | what it is | size |
|---|---|---|
| **Fury** | the application: profiles, proxies, personas, teams | ~12 MB |
| **Fury core** | the browser itself — a Chromium fork | 103 MB on macOS, 146 MB on Windows |

They are apart because they move apart. The application changes weekly; the core
changes when Chromium does, every six weeks or so. Bundling them would mean a
100-plus MB download every time a button moved. It also keeps the application's code
signature intact — a core written into a signed bundle breaks its seal, after
which macOS says the application is *damaged*, which sends you looking for a
corrupt download rather than at us.

## macOS

### 1. The application

Download `fury-<version>-macos-<arch>.dmg` from
[Releases](https://github.com/furyteamtop/fury-antidetect-browser/releases). `arch` is `arm64` for
any Apple-silicon Mac (M1 and later) — if you are not sure,  → About This Mac
says which. Intel Macs have no release build yet: the name would be `x86_64`,
and until one is published the way in on an Intel machine is to build it.

Open it and drag **Fury.app** into `/Applications`, the way every other Mac
application is installed.

### 2. The browser core

Open Fury. It says the browser itself is not installed yet and offers
**Download the browser** — press it. That fetches the core for this machine
from the Releases page, about 100 MB, once, and installs it. Nothing runs on a
timer or at startup: the request is made when you press the button and not
otherwise, which is the difference between this and the automatic updates the
project refuses to have (the reasons are at the bottom of this page).

Or by hand: download `fury-core-<version>-macos-<arch>.tar.xz` from the same
page, then in Terminal:

```bash
/Applications/Fury.app/Contents/MacOS/fury-agent install-core ~/Downloads/fury-core-*.tar.xz
```

The button runs exactly this. It does three things by hand that are easy to get
wrong: it unpacks the bundle preserving the symlinks a macOS framework is built
from, it removes the quarantine flag your browser attached to the download, and
— the step that matters — it **runs the browser once and checks it starts**.
If it prints a version, it is installed:

```
installed Fury 153.0.8010.37
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
in **Settings → Team server**, which is also where you create an account and
where the instructions for running your own server are. The sign-up screen
comes with the project's open server already filled in; a team with a server of
its own replaces the address. That is a decision you can make
later, or never.

### Verifying what you downloaded

Every release has a `SHA256SUMS` file. Download it next to the archives and:

```bash
shasum -a 256 -c SHA256SUMS
```

Two `OK` lines mean the files are what was published.

### If macOS refuses to open it

**It should not, from 0.1.4's re-upload of 21.09.2026 on.** The application,
the core and the disk image are signed with a Developer ID and carry a stapled
notarisation ticket; `spctl --assess` on a machine that has never seen them
answers `accepted, source=Notarized Developer ID`. If a current download is
refused, the file is not what was published — check it against `SHA256SUMS`
before anything else.

What follows is for **0.1.3 and earlier**, which were ad-hoc signed, and for
a bundle you built yourself.

**"Fury cannot be opened because the developer cannot be verified."** This is
the normal result for an unsigned application, and it is what you should expect
for the *correct* file — an earlier version of this page told you to download it
again, which was a loop with no exit. Instead:

  1. Try to open it once and dismiss the dialog. Then System Settings →
     Privacy & Security, scroll to the bottom: next to *"Fury" was blocked* press
     **Open Anyway** and confirm. macOS remembers the choice for that copy.
     (Right-click → **Open** used to be enough; since macOS 15 it is not.)
  2. Or strip the quarantine flag in Terminal, which is what the dialog is
     really about:

     ```bash
     xattr -dr com.apple.quarantine /Applications/Fury.app
     ```

The same applies to the core: `fury-agent install-core` removes the flag from
what it unpacks, which is why the core needs no step of its own here.

**"Fury is damaged and can't be opened."** On macOS 15 and later this is the
wording you get for an ad-hoc signed download, so it is the same case as above:
strip the quarantine flag and it opens. The release 0.1.2 bundle passes
`codesign --verify --deep --strict` on the machine it was built on, so the
signature itself is sound. If the message survives the `xattr` step, then the
seal is genuinely broken — something was added to or removed from inside
`Fury.app` — and the fix is to delete it and unpack the download again.

**A core you built yourself will not start**, with a message about *Team IDs*.
That is a signing arrangement rather than a broken build; see
[tools/release/sign-core.sh](../tools/release/sign-core.sh), which explains it
in full.

## Windows

Works, as of 16.08.2026, and in the releases since 0.1.1. An earlier version of
this section said "not yet" for as long as that was true: the launcher was
ported first, and the core — the Chromium build — was the part nobody had run on
a Windows machine. It has been run now, and `tools/verify-windows.ps1` passes
30 claims against it on a real machine. The same two downloads as on macOS.

### 1. The application

Download `fury-<version>-windows-x64-setup.exe` from
[Releases](https://github.com/furyteamtop/fury-antidetect-browser/releases)
and run it. **SmartScreen will object** — "Windows protected your PC" — because
the installer is not signed, not because anything is wrong with it. Press
**More info**, then **Run anyway**.

The installer asks for nothing: it installs for the current user, into
`%LOCALAPPDATA%\Fury`, and needs no administrator password. That directory
holds `fury-desktop.exe`, `fury-agent.exe` and the uninstaller, and nothing
else.

### 2. The browser core

Open Fury and press **Download the browser**, the same button as on macOS: it
fetches `fury-core-<version>-windows-x64.tar.xz` from Releases, about 150 MB,
once, and installs it. Or by hand — download that file from the same page and,
in PowerShell:

```powershell
& "$env:LOCALAPPDATA\Fury\fury-agent.exe" install-core "$env:USERPROFILE\Downloads\fury-core-<version>-windows-x64.tar.xz"
```

On Windows the install step reads the version out of the executable instead of
running it: `chrome.exe --version` starts a browser and never exits.

### 3. Open it

Open Fury from the Start menu. Everything the macOS section says about accounts
applies: there are none until you want a team.

**Widevine** — Netflix, Spotify and the other DRM sites — is not in the
download, because it cannot be redistributed. The agent stages it out of the
Chrome already installed on the machine; a machine with no Chrome gets a
working browser without DRM, and a site that checks for it will notice.

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

On Windows the same directory is `%APPDATA%\Fury` — Roaming rather than
Local, deliberately: on a domain-joined machine it follows the user, and
losing the database on a different machine would look like data loss rather
than a cache miss.

Set `FURY_HOME` to put it somewhere else — an external disk, or an encrypted
volume.

To remove Fury completely: delete that directory and `/Applications/Fury.app`
— on Windows, that directory and Fury from Settings → Apps.
Nothing is written anywhere else, and nothing is left behind on any server you
did not set up yourself.

## Updating

Download the new `.dmg` and drag the application over the old one. For a new core, run
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
