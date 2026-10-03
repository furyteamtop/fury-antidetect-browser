# Linux

The Linux port, x86_64, as of 0.2.16. What ships, how it is built, and what is
still honest to call unverified. The first measured evidence exists — a core
built on Ubuntu 22.04 (WSL on a Windows machine) that answers `--version` with
the pinned version — but "it works" is a claim about a running browser on a
real desktop, and that acceptance has not run yet. The status section at the
bottom separates the two.

## What ships

Three artifacts, infix `linux-x64` everywhere — the same string
`agent/src/core_download.rs` asks the releases API for, so a release made by
these scripts is one the shell's own download button can find:

    fury-<version>-linux-x64.AppImage     # the shell and the agent, portable
    fury-<version>-linux-x64.deb          # the same, installed system-wide
    fury-core-<version>-linux-x64.tar.xz  # the patched Chromium, flat archive

The shell ships without the browser, exactly as on macOS and Windows: the core
is 134 MB compressed against the shell's 12 MB and versioned on its own
schedule. The shell downloads the matching core from the releases page when
asked, or a core tar.xz can be fed to `fury-agent install-core` by hand.

The core archive is **flat**: `chrome` and its support files at the top, no
wrapping directory. That is the shape the agent's `core_binary()` looks for —
it joins its leaf names (`fury-core`, `chrome`) directly onto the install
directory, so an archive with a top-level directory would install and then
never launch. `tools/release/pack-core-linux.sh` enforces this and says so in
its comments; `tools/ci/check-linux-release.sh` pins the naming agreement on
every CI run.

## Install

    sudo apt install ./fury-<version>-linux-x64.deb

or make the AppImage executable and run it. Then open Fury and press the
download button, or:

    fury-agent install-core ~/Downloads/fury-core-<version>-linux-x64.tar.xz

Everything Fury owns lives under `${XDG_DATA_HOME:-~/.local/share}/fury` (the
agent and the shell derive the same path from `fury_platform::dirs`, which is
the one function both speak to).

Dependencies are the ones Tauri declares for the deb (WebKitGTK 4.1 and its
GTK stack). The AppImage bundles what it needs except the GPU driver stack.

## Display servers: Wayland and X11

Both Ozone backends are compiled into the Linux core
(`ozone_platform_x11`/`ozone_platform_wayland` in `core/args/linux-x64.gn`);
the choice happens at launch:

- **X11 session** (no `WAYLAND_DISPLAY`): no switch is passed, and Chromium's
  default X11 path runs, with GPU and sandbox exactly as upstream builds them.
- **Wayland session** (`WAYLAND_DISPLAY` set — sway, Hyprland, GNOME/KDE on
  Wayland, including sessions that also run XWayland): the launcher adds
  `--ozone-platform=wayland`. Native Wayland is the default on every session
  that offers it, because the native backend follows the compositor's scaling
  directly — the whole game on HiDPI setups. On a compositor running without
  XWayland the switch is also what makes the core start at all: its
  compiled-in default is X11, and there is no X server. Compositors that
  export both sockets, Hyprland among them, still get the native backend.
- **Override**: set `FURY_OZONE_PLATFORM=wayland` (or `x11`) in the agent's
  environment to force a backend; it wins over the inference above. On a
  Wayland session, `x11` is the explicit XWayland compatibility path.

Nothing in the port passes `--disable-gpu`, `--no-sandbox` or any other
global concession. GPU and sandbox are upstream's, on every backend. The
Chromium sandbox uses unprivileged user namespaces, which is the default on
current distributions; a distro that disables them needs its own arrangement
(upstream documents it; we do not ship a setuid helper by default).

The shell (Tauri/WebKitGTK) is a separate renderer with its own display
handling. The AppImage leaves the host Wayland client/EGL libraries in charge
and selects Wayland when `WAYLAND_DISPLAY` is present; this avoids a startup
abort caused by mixing the build image's Wayland stack with the host driver.
On some proprietary-driver setups WebKitGTK's DMABUF path can still render
blank. If that happens, run once with `WEBKIT_DISABLE_DMABUF_RENDERER=1` — it
is scoped to the shell and never touches the browser core.

## Building

Core — on a Linux x86_64 machine with the usual Chromium resources (the
self-hosted builder runs this via `.github/workflows/linux-core.yml`):

    core/build/fetch.sh "$(cat core/CHROMIUM_VERSION)"
    core/build/apply.sh
    core/build/build.sh linux-x64
    tools/release/pack-core-linux.sh     # -> dist/fury-core-<version>-linux-x64.tar.xz

The packer refuses to ship a PGO-instrumented build (the same check
pack-core-windows.sh makes), fails loudly on any missing runtime file, and
proves the staged core by running `chrome --version`, which on Linux needs no
display server.

Shell — on a Linux x86_64 machine with `libwebkit2gtk-4.1-dev`,
`libayatana-appindicator3-dev`, `librsvg2-dev`:

    tools/release/package-linux.sh [version]   # FURY_CORE_DIR=<staged core dir>

It runs the workspace tests, builds the AppImage and deb via tauri, names the
three artifacts, and packs the core archive. Without `FURY_CORE_DIR` it
refuses: a release must carry the core it was tested against. The version
argument must equal the one in `desktop/package.json` — the deb embeds it, and
a mismatch is rejected rather than shipped under a renamed file.

## Releasing

`.github/workflows/linux-release.yml` (workflow_dispatch) assembles and
verifies the three artifacts on a hosted runner: workspace tests, core
download over HTTPS, packaging, then package verification (ELF header,
dpkg metadata, archive contents).

There is **no automatic publish**. The workflow stops at uploaded artifacts;
attaching them to a release is a person's step:

    gh release create "v<version>" dist/* --prerelease --generate-notes

No checksum files are produced by these scripts. Whether a release carries a
SHA256SUMS is a decision that belongs to the release flow that publishes it,
and that flow is not part of the port.

## CI

- `cargo test (everything but the shell)` — runs on Linux already; the Linux
  branches of the agent and platform crates are tested there, including the
  launch-argument and asset-name contracts added with this port.
- `cargo test (the shell, on Linux)` — new: compiles and tests fury-desktop
  against the WebKitGTK stack it actually runs on. Until this job existed,
  the Linux shell was compiled nowhere, which is exactly the blind spot the
  Windows surface job was built to prevent.
- `the Linux release contract holds` — new: tools/ci/check-linux-release.sh,
  static agreement of every producer and consumer of the artifact names.
- The browser itself is checked by `core/verify/run-all.py` on the
  self-hosted Linux builder (linux-core.yml), same split as macOS and
  Windows.

## Status and known limits

Measured so far: the core compiles for `linux-x64` — the first one was built on
Ubuntu 22.04 (WSL on a Windows machine) — and the binary answers
`chrome --version` with the pinned version, 155.0.8059.12;
`tools/release/pack-core-linux.sh` completed its first real pack run and
produced the archive; a sandboxed smoke run on an owned page passed.

Implemented and statically verified on the source level: GN args, build
target, packer, packager, workflows, launcher selection, contracts, docs. The
existing wrong-Chrome check also works on Linux now: Linux has no version
resource to read, so the agent reads the version with a bounded `--version`
run and, as on macOS and Windows, reports an installed core of a different
Chrome major as not installed for the download to replace it.

Still unverified, and not claimed until it is:

- an actual AppImage/deb install — the packaging scripts exist, the acceptance
  run has not happened;
- a profile running under Wayland and under XWayland on a real compositor,
  including GPU and Widevine (on Linux the CDM arrives through the component
  updater, since bundling it is redistribution);
- the detect-suite captures, which are macOS/Windows measurements; nothing
  here claims Linux parity of the fingerprint, only that the platform is
  served by the same patch series.

See docs/03-chromium-fork.md for the patch series itself; the Linux decision
recorded there (04.08.2026, Linux excluded) is superseded by this port.
