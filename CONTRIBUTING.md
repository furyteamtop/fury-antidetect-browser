# Contributing

The short version: measurements are welcome, claims are not.

This project's whole method is that every statement about the browser has a
number behind it, taken from a browser that was actually running. A patch with
a measurement is easy to accept even if the code needs work. A patch without one
cannot be accepted at all, however good it looks, because nobody can tell
whether it does anything.

## The most useful thing you can send

**A persona from your computer.** The catalogue has 26 machines and every one is
a crowd for somebody to hide in. If your machine is not in it — a different GPU,
a different screen, a Linux laptop, an older Windows — that is the highest-value
contribution there is, and it takes one command.

Run the probe in an ordinary browser, not Fury:

```bash
cd tools/detect-suite && python3 collector.py
```

Open `http://127.0.0.1:8731/probe.html` in Chrome, then:

```bash
cargo run -p fury-detect -- persona baselines/<your-capture>.json > persona.json
```

It refuses rather than guesses, so a file that comes out is a file that will
launch. Read it before you send it: a persona describes YOUR computer — its
GPU, screen, fonts, audio latency and installed speech voices.

## Reporting a detection

If a site catches a Fury profile, that is the most valuable bug report this
project can get, and the useful form of it is:

1. What the site is (or, if you would rather not say, what it checks).
2. A capture from the Fury profile, redacted:
   `cargo run -p fury-detect -- redact <capture>.json <capture>-redacted.json`
3. The same capture from real Chrome on the same machine, if you can.

`fury-detect diff` between those two is usually the whole diagnosis.

Please do not send account credentials, and do not send a raw capture — it
contains the public IP your proxy exits from.

## Patches to the browser

`core/patches/` is a quilt-style series against Chromium 150.0.7871.187. Read
[`core/patches/series`](core/patches/series) first — it is the design record,
and it explains not only every patch but every patch that was written and then
**retired** after measurement showed it would make the browser less like Chrome,
not more.

The rules that get a patch accepted:

**One patch, one vector.** At rebase time a conflict has to be localisable to a
named concern.

**Measure before you write.** Three patches in this series were retired because
the measurement said they were unnecessary: navigator.plugins (byte-identical to
Chrome already), CSS media features (zero differences across 21 probes), and
TLS/JA3 (BoringSSL randomises extension order per connection, so Chrome has no
stable fingerprint to match). A patch that makes Fury *differ* from Chrome is
worse than no patch.

**"It applies" and "it compiles" are not "it works."** Patch 0082 applied,
linked, and answered every geolocation call with a timeout, because macOS asks
the OS for permission before it asks any provider. Patch 0110 typechecked in
every language it was written in and did not compile. Both were caught by
running the browser, and neither was visible in review.

So: a patch needs a script in [`core/verify/`](core/verify/) that drives the
built browser over CDP and asserts what the patch claims. Read
[`core/verify/README.md`](core/verify/README.md) — it describes how to write
one, including the two ways this project's own harnesses have lied.

**The cross-context number is the gate.** Any patch has to leave
`crossContext.disagreements` at zero across all eight contexts — main frame,
dedicated Worker, SharedWorker, ServiceWorker, AudioWorklet, and three kinds of
iframe. A spoof that reaches the main frame and forgets `new Worker()` is caught
in one line by any serious checker.

```bash
cargo run -p fury-detect -- gate <capture>.json
```

Thirteen checks; all must pass.

## Everything else

`agent/`, `server/`, `desktop/`, `shared-rs/` are ordinary Rust and TypeScript.

```bash
desktop/scripts/sidecar.sh      # once, before the first cargo test
cargo test --workspace          # 253 tests
cd desktop && npm run build     # typechecks and bundles the shell
```

The first line surprises people, so: `desktop/src-tauri/tauri.conf.json`
declares the agent as a Tauri sidecar, and `tauri-build` looks for it at
`binaries/fury-agent-<target-triple>` while running the shell's build script.
That directory is gitignored — a 7 MB binary per platform has no business in
the history — so on a fresh clone the shell does not compile and says

```
resource path `binaries/fury-agent-aarch64-apple-darwin` doesn't exist
```

which names a path that has never existed on your machine and does not mention
the script that creates it. `sidecar.sh` builds the agent and puts it there,
and it is the same script a release uses.

It went unnoticed until the first CI run, because every machine that had ever
built the shell already had one.

Skip it and `cargo test --workspace --exclude fury-desktop` still works, which
is what CI runs on Linux.

All three must pass. There is no formatter check and no lint gate — match the
surrounding code instead, which is more specific than any rule set: comments
explain *why*, cite `file:line` for claims about upstream code, and record what
was measured rather than what was assumed, including what was got wrong first.
The commit log is the reference for the tone.

## Things that will be turned down

- A fingerprint value invented rather than measured. `shared-rs/src/persona.rs`
  opens with why: a competitor derives `hardwareConcurrency` from a seed and
  hardcodes `deviceMemory`, so it reports machines that do not exist, and a
  valid-but-impossible combination is a *stronger* signal than no spoofing.
- A patch that widens a capability rather than narrowing one. Removing a WebGPU
  feature is honest — `requestDevice()` then fails as it would on hardware that
  lacks it. Adding one advertises something the driver cannot do and fails in a
  way no real machine fails.
- Anything that makes the product easier to use for fraud specifically, as
  opposed to for the legitimate multi-account work described in
  [`ACCEPTABLE_USE.md`](ACCEPTABLE_USE.md).

## Licensing

By sending a patch you agree it is licensed under the terms of the directory it
lands in: **BSD-3-Clause** for `core/patches/`, **Apache-2.0** for `shared/`,
and **AGPL-3.0-or-later** for everything else. See [`NOTICE`](NOTICE).

No CLA. Keep your copyright.

## Getting hold of me

[@shapovalovbogdan](https://t.me/shapovalovbogdan) on Telegram, or the issue
tracker. A detection report is worth interrupting me for.
