<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.png">
  <img src="assets/logo.png" alt="Fury" width="420">
</picture>

A free, open-source anti-detect browser with real team collaboration. Own
Chromium fork, works standalone with no server, self-hostable when you need a
team. No seats, no per-profile pricing, no telemetry.

**[furybrowser.dev](https://furybrowser.dev)** · *[Русская версия](README.ru.md)*

> **Status: in development, and now on both macOS and Windows.** The core builds
> and spoofs; the agent launches profiles; the server and desktop shell work.
> Builds are on the [Releases](https://github.com/furyteamtop/fury-antidetect-browser/releases)
> page, marked pre-release. **macOS builds are signed and notarised since
> 21.09.2026; Windows is not** — see below.
>
> **Windows works, as of 16.08.2026.** The Chromium core built (57 528 targets),
> the agent runs on it, and `tools/verify-windows.ps1` passes 30 claims on a real
> machine: the config reaches the browser as an inherited HANDLE, argv carries a
> slot number and nothing else, no process in the tree has a persona string in
> its command line, and the browser reports the persona's platform rather than
> the host's. The desktop shell builds to an NSIS installer. This paragraph used
> to say *not yet*, and it said so for as long as that was true.
>
> Getting there found eight defects that only running could find — a BSD `df`
> flag, a bootstrap that returned success having done nothing, CRLF making
> `git apply` claim a patch was stale, PowerShell reading UTF-8 as
> Windows-1252, a persona leaking through `--user-data-dir` into every child
> process, and a `beforeBuildCommand` that cmd.exe could not execute. They are in
> the history, one commit each, with what they cost.
>
> **Team mode works end to end, as of 18.08.2026.** Invite, enrol on a second
> machine, be let in with one button, send a profile to the server, open it
> there. Every stage of that path had a defect and one of them — sending a
> profile — had never worked at all: the fingerprint seed crossed as a number
> where the server wants sixteen hex characters, so every upload since the
> endpoint existed died in the request parser. They are in the history with what
> each looked like from the operator's side.
>
> **macOS is signed and notarised, as of 21.09.2026.** The Apple Developer
> enrolment paid for on 15.08.2026 came through that day, and the same evening
> the 0.1.4 macOS files were rebuilt with the Developer ID and re-uploaded:
> the application, the core and the disk image each carry a stapled ticket,
> and Gatekeeper's verdict on a machine that has never seen them is
> `accepted, source=Notarized Developer ID`. Open the DMG, drag, open — no
> right-click, no `xattr`. The first real run of
> [sign-core.sh](tools/release/sign-core.sh) and
> [sign-shell.sh](tools/release/sign-shell.sh) found five defects in the
> scripts themselves, which is in the history; [docs/17](docs/17-apple-signing.md)
> is the paperwork.
>
> **Windows is not signed.** That is a separate certificate, not started, so
> the installer shows SmartScreen: **More info → Run anyway**. A macOS download
> of 0.1.3 or earlier is ad-hoc signed and needs `xattr -dr
> com.apple.quarantine /Applications/Fury.app` once; [docs/15](docs/15-install.md)
> walks through both.
>
> **Linux** is not a target. See the table at the bottom.
>
> Everything that is *not* done is listed at the bottom, honestly.

## Why another one

Anti-detect browsers cost $30–150 a month per team and are closed source, so
nobody can check what they actually spoof. We measured the popular ones: in one,
the canvas readback was **byte-identical to a clean machine** — not spoofed at
all, while the interface listed it as protected
([docs/08](docs/08-competitors.md)).

This is the inverse. Every vector is documented against the Chromium file it
lives in, the patches are in the repo, and you can measure it yourself with the
bench that ships alongside.

"Self-hosted" is a word others use now too — with a closed core behind a licence
key and one token for the whole installation. Here three things come together,
each of which exists somewhere on its own and nowhere together: **a core you can
build and verify**, **a team** with roles, locks and an audit trail, and **a
server you run yourself** that cannot read your bundles
([docs/08](docs/08-competitors.md)).

## Three ways to run it

| | What you host | For |
|---|---|---|
| **Solo** | nothing | one person, own profiles |
| **Team, self-hosted** | one binary + PostgreSQL | an agency or a team |
| **Team, hosted for you** | nothing | people who do not want to run servers |

**Solo is the default.** No account, no registration, no database. Profiles and
proxies live on your machine. Everything that makes this an anti-detect browser
works here in full — the team layer adds nothing to the fingerprint.

**Self-hosted** is what you turn on when there is a team: projects, per-project
access grants, a distributed lock. One binary and Postgres on any VPS —
[docs/13](docs/13-self-hosting.md).

**Hosted** exists, and it opened only once the condition it was gated behind
was met: bundles are encrypted on the operator's machine, so the server holds
data it cannot itself read. The address is prefilled on the sign-up screen
(`srv.furybrowser.dev`, [desktop/src/defaults.ts](desktop/src/defaults.ts))
and is editable — a team with its own server clears it and types theirs. It
runs with open sign-up, every registration is its own organisation, and it is
still somebody else's machine: [docs/13](docs/13-self-hosting.md) is one
command if you would rather that not be true.

## What works today

A profile using a Windows 11 / RTX 4060 persona, launched on an Apple Silicon
MacBook, reports:

```
navigator.platform      Win32
userAgent               Windows NT 10.0; Win64; x64 … Chrome/153.0.0.0
WebGL renderer          ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 …)
screen                  1920×1080, availHeight 1032   ← the taskbar
Client Hints platform   Windows        brands: … Google Chrome/153
timezone                Europe/Berlin
navigator.webdriver     false
```

Consistently — in the main frame, in a Worker, and in three kinds of iframe. A
disagreement between execution contexts is three lines of JavaScript to find and
gives away a spoof more reliably than not spoofing at all.

The noise is deterministic: the same profile produces the **same** canvas hash
on every read, forever. A fingerprint that changes between calls describes a
machine whose hardware moves while you watch it, which is worse than an honest
one.

## How it fits together

```
desktop (Tauri)  ──socket──▶  agent (Rust)  ──spawn──▶  core (Chromium fork)
     │                           │                        through a relay
     └──HTTPS──▶ server (optional: teams)
```

- **core** — Chromium 153 fork, [28 patches](core/patches/); spoofing is in C++,
  never injected JavaScript
- **agent** — the only component holding decrypted secrets: proxy relays,
  launching, the local automation API
- **server** — organizations, projects, permissions, locking. Deliberately dumb:
  it never generates a fingerprint and never decrypts a bundle
- **desktop** — Tauri rather than Electron: a 7 MB app, not 120 MB

Details in [docs/01](docs/01-architecture.md).

## Install

If you only want to use it, you do not need any of what follows:
[docs/15](docs/15-install.md) is two downloads and one command, and it is the
page to send anybody who asks how to try this.

## Build

The core took **2 h 42 min** on an Apple M5 with 10 cores and 16 GB, in the
`macos-arm64-lowmem` configuration — measured on 30.07.2026, not estimated. The
checkout and one build directory come to ~39 GB, also measured; `fetch.sh` still
demands 150 GB free and warns about ~100 GB, and that number is a cautious
estimate rather than a measurement ([docs/03](docs/03-chromium-fork.md)).
Everything else takes minutes. Incremental rebuilds after the first are 5-30
minutes depending on what changed; `ccache` and `sccache` do not help, because
the build uses `-fmodules` and they miss on everything.

```bash
git clone https://github.com/furyteamtop/fury-antidetect-browser && cd fury
cargo build --release
```

```bash
cd desktop && npm install && npm run app:build
```

Building the core: [docs/03](docs/03-chromium-fork.md).

## Run it

Start the local daemon — this is the whole solo product:

```bash
cargo run -p fury-agent -- serve
```

Point it at a core binary if it is not beside the agent:

```bash
FURY_CORE=/path/to/Chromium cargo run -p fury-agent -- serve
```

Launch a persona directly, without any store, to see the spoofing work:

```bash
cargo run -p fury-agent -- launch shared/personas/windows-11-rtx4060-1920x1080.json --proxy socks5://user:pass@exit.example:1080 --timezone Europe/Berlin
```

## Automation

Six endpoints and one bearer token, off until `FURY_API_PORT` says otherwise.
[examples/](examples/) has four working scripts — curl, Playwright, Puppeteer,
and the one people actually want: run a job across every profile, one at a time,
never leaving a browser open on a failure.

**MCP server — [tools/mcp/fury-mcp.py](tools/mcp/fury-mcp.py).** The same
operations for Claude Desktop, Cursor and any MCP client: "open every profile
tagged warm, visit the platform, close them". Competitors sell this as an "AI
agent" — for credits, on their model, on their server, with access to your
profiles. Here the agent is yours, the key is yours, and nothing about a profile
leaves the machine. Not less convenient; fewer intermediaries.

**Extensions** — a `.crx` is installed into a profile from the application; the
extension's id survives cloning (the developer key is written into the
manifest), so a wallet or an anti-captcha stays signed in inside the copy.

**Domain lists** — a per-profile blocklist enforced by the relay (DoH cannot
route around it), or a whitelist: put `@allow-only` on the first line and the
profile opens only the platforms named. For the team member who should be on
one.

**Fingerprint probe** — every profile's start page links to a full detect-suite
run inside that profile: every vector, every execution context, in a secure
context. The same probe the baselines are captured with, not a separate display.

## Check it yourself

Do not take any of the above on trust — measure it:

```bash
cd tools/detect-suite && python3 -m http.server 8791
```

[`tools/detect-suite/status.html`](tools/detect-suite/status.html) is the last
measurement rendered as a page — the gate's thirteen checks, the nine contexts,
and Fury beside real Chrome on the same machine. It is generated from the
captures in the repository, so it cannot say anything they do not.

Open `http://127.0.0.1:8791/probe.html` in ordinary Chrome and in Fury, and
compare the dumps. `fury-detect diff` shows what moved, `fury-detect gate` runs
the release criteria and exits non-zero on failure, so it drops into CI
([tools/detect-suite](tools/detect-suite/README.md)).

## Documentation

| Doc | Contents |
|---|---|
| [01 — Architecture](docs/01-architecture.md) | Components, processes, deployment tiers |
| [02 — Fingerprint surface](docs/02-fingerprint-surface.md) | Every vector, where it lives, where to patch it |
| [03 — Chromium fork](docs/03-chromium-fork.md) | Building, patch management, the rebase treadmill |
| [04 — Data model & RBAC](docs/04-data-model-rbac.md) | Schema, roles, permission matrix |
| [05 — Proxy & networking](docs/05-proxy-networking.md) | Relay design, DNS, WebRTC, leak prevention |
| [06 — Profile sync](docs/06-profile-sync.md) | Bundle format, encryption, locking, conflicts |
| [07 — Detection baseline](docs/07-detection-baseline.md) | Test harness and measurable pass criteria |
| [08 — Competitors](docs/08-competitors.md) | What we measured in the commercial ones |
| [09 — Roadmap](docs/09-roadmap.md) | Phases with exit criteria |
| [16 — Parity and beyond](docs/16-parity-and-beyond.md) | What four competitors have that this does not, what none of them have, and the order to do it in |
| [10 — Legal & licensing](docs/10-legal-licensing.md) | Chromium BSD, Widevine, branding, code signing |
| [11 — Budget](docs/11-budget.md) | What costs money and what does not |
| [12 — UX reference](docs/12-ui-reference.md) | What to copy from AdsPower, and where to beat it |
| [13 — Self-hosting](docs/13-self-hosting.md) | Standing up a team server |
| [14 — Team server](docs/14-team-server.md) | Accounts, enrolment, the RBAC model in practice |
| [15 — Installing](docs/15-install.md) | For somebody with no toolchain ([ru](docs/15-install.ru.md)) |
| [17 — Apple signing](docs/17-apple-signing.md) | Getting the Developer ID certificate that task 0.2 waits on, and what still has to be written once it exists |

Documents are in Russian except 15, which is the one a downloader reads;
translation of the rest is planned.

## Not done yet

Three lists, kept apart on purpose: what is still open, what was decided
against and why, and what has been closed. A closed item stays on the page
with the measurement that closed it, because the next person to ask "does it
handle X" deserves the answer and not the archaeology.

### Open

| | |
|---|---|
| Code signing, Windows | not started. A separate certificate (EV or OV) and a separate process; until then the installer shows SmartScreen and the way through is **More info → Run anyway** |
| Widevine on a machine with no Chrome | the agent stages the CDM out of the Chrome already installed on that machine, so nothing proprietary is redistributed and `com.widevine.alpha` is answered the way real Chrome answers it. A machine with no Chrome at all gets a working browser with no DRM, which is detectable |
| Persona catalogue | 27 machines — the 27th arrived through the issue form on 13.09.2026, a Windows 10 desktop with a GTX 950. More personas means better crowds to hide in, and it is the most useful thing an outside contributor can add — `fury-detect persona <capture.json>` turns a probe capture from your own computer into one |
| Speech synthesis voices | a leak, found 24.09.2026. A persona of another OS reports the host's voices: a Windows persona on a Mac answers `speechSynthesis.getVoices()` with 180 macOS voices ("Milena", "Eddy (…)"), which no Windows machine has. The built-in personas carry `voices: []`, so the filter never engages; the contributed Windows 10 one lists only Google voices, which Fury does not have, and patch 0041 discards a filter that matches nothing, because an empty list gives the spoofing away even more surely. The patch can only narrow the list. A voice the system does not have cannot be added: `speak()` on it would fail in a way no real machine fails. So on a Mac a Windows persona cannot get Windows voices at all; until this changes, the rule is a persona of the host's OS. CreepJS and the Castle checks do not catch it, they only look for an empty list |

### Decided against, with the reason

Each of these is a way to be caught or a way to be inconvenienced, and the
row says why the alternative is worse.

| | |
|---|---|
| Linux | not a target, and this is a decision rather than a gap. The Rust still compiles there so CI and contributors can run the suite; there is no Linux release, no Linux core config and no plan for one. Shipping a third platform nobody tests would be a claim, not a port |
| WebRTC through the proxy | no. The relay is TCP; patch 0070 puts the browser in the state a real Chrome reaches under the enterprise `WebRTCIPHandlingPolicy` — no ICE candidates at all — rather than let a peer connection go around the proxy and hand the page the real address |
| Hiding CDP from a timing check | no, and now known to be unclosable rather than merely undone. Split into its two parts ([cdp-timing.py](tools/detect-suite/cdp-timing.py)): attaching costs nothing, `Runtime.enable` costs a fixed 2.7x plus more as the logged object grows. A patch can remove the size half — preview generation — and not the fixed half, which is the message reaching the frontend at all. Real Chrome measures the same. The control that works is `cdp: false`, which is the default |
| Automatic updates | none, and deliberately so: an updater is a scheduled channel into an anti-detect browser from an address that is not the profile's proxy. [docs/15](docs/15-install.md) says what updating looks like meanwhile |
| Offline GeoIP | no, and it is a dependency rather than a leak. The exit check asks ipinfo.io **through the proxy**, so what the third party sees is the exit's address and never the operator's — asserted by a test that points the check at a dead proxy and requires it to fail rather than answer. `checker_url` per proxy and `FURY_IP_CHECK` let you point it at your own. An embedded database would remove the dependency and costs 60+ MB and a licence to redistribute |
| QUIC / HTTP-3 | no difference from Chrome, but it does show the proxy. Measured on three sites: real Chrome with no proxy uses h3; real Chrome behind a SOCKS5 proxy uses h2 and never h3. Chromium does not carry QUIC through a proxy, and a profile is always behind one, so Fury behaves exactly like Chrome behind the same proxy. That is not the same as looking like a home user: a site that offers h3 and sees a client stay on h2 visit after visit can conclude that UDP does not reach it, meaning the client is behind a proxy or on a network that blocks UDP. The signal is weak (h2 is also normal on office networks and on a first visit, before the browser learns about h3), but it exists and Fury does not hide it. Hiding it takes a proxy that forwards UDP, which Chromium cannot do through `--proxy-server`; turning that on would make Fury *differ* from Chrome behind a proxy |

### Closed

| | |
|---|---|
| ~~Windows core build~~ | done 16.08.2026: the core builds on the build server, release `v0.1.2` ships `fury-core-0.1.2-windows-x64.tar.xz` on Chromium 153, `verify-windows.ps1` passes 30 claims, Widevine answers |
| ~~Windows launcher~~ | done: agent and shell run, NSIS installer in the releases. The config reaches the browser as an inherited HANDLE and no persona value appears in any argv — checked on a live machine |
| ~~Code signing and notarisation, macOS~~ | done 21.09.2026: Developer ID, notarised, stapled — application, core and disk image, [sign-core.sh](tools/release/sign-core.sh) and [sign-shell.sh](tools/release/sign-shell.sh). Verified on the files downloaded back from the release page with the quarantine flag set |
| ~~Client-side bundle encryption~~ | done, and verified end to end against a running server: what it writes to disk holds neither the cookie, nor a tar header, nor a gzip header, and a foreign organisation key does not open it |
| ~~Bundle sync with the server~~ | done. Packed and sealed on stop, fetched and unpacked on launch, versioned so a second uploader is refused rather than silently winning. Uploads stream to disk: they used to buffer, under axum's 2 MB default, which meant sync had never once worked for a real profile |
| ~~Shared TOTP secrets~~ | done, both modes. A profile carries logins — username, password, two-factor seed — sealed with the machine key alone, or with a per-login data key wrapped under the organisation key when there is a server. The server holds a blob it cannot read; a foreign organisation asking for it gets 404 from the handler and zero rows from the database. All of RFC 6238's vectors pass, for SHA-1, SHA-256 and SHA-512, and the code is computed outside the webview in both modes |
| ~~Per-organisation quotas~~ | done, and off unless set. `FURY_MAX_ORGS`, `FURY_MAX_PROFILES_PER_ORG`, `FURY_MAX_STORAGE_PER_ORG` — for a server that takes open sign-ups, where isolation between organisations is total and fairness is not |
| ~~Row-level security on the server~~ | done. Migration 0006 adds FORCE (the app owns its tables, and an owner is exempt without it) and `auth::Db` binds the caller to the connection. Verified against a real PostgreSQL — remove either half and four tests fail |
| ~~The "Google API keys are missing" bar~~ | done 22.09.2026, patch 0902, in the 0.1.6 core. Chromium showed it in the first window of every profile, in the UI language, saying "Chromium"; writing the check found it also left `innerHeight` 56 px larger than the viewport the page laid out in — a first-window mismatch real Chrome does not have. `core/verify/verify-0902.py` measures both |

## Contributing

The most useful thing you can send is a persona from your own computer —
`fury-detect persona` turns a probe capture into one, and the catalogue is 27
machines, each of which is a crowd for somebody to hide in. The second most
useful is a site that caught a profile. [CONTRIBUTING.md](CONTRIBUTING.md) has
both, and the rule that governs everything else: measurements are welcome,
claims are not.

## Licensing

Three sets of terms, and they are not interchangeable.

- `agent/`, `server/`, `desktop/`, `shared-rs/`, `tools/`, `core/build/`,
  `core/args/`, `core/verify/` — **AGPL-3.0-or-later** ([LICENSE](LICENSE))
- `core/patches/` — derived from Chromium, **BSD-3-Clause**, upstream terms
  preserved, so a browser built from the series carries one set of terms and no
  copyleft reaches someone who only wants the browser
- `shared/` schemas — **Apache-2.0**, so anyone can implement compatibility

AGPL is deliberate: anyone running a service on this code has to publish their
changes, which is what keeps "free" free. Reasoning in
[docs/10](docs/10-legal-licensing.md).

Chromium itself is not in this repository — `core/build/fetch.sh` downloads it
from Google under its own licence.

**Widevine is not distributed here and must not be.** The CDM is a proprietary
binary. The low-memory GN args build with Widevine support and
`core/build/link-widevine.sh` stages the blob out of the Chrome already
installed on the build machine — which is fine because it is already there, and
only because it never leaves. A bundle built that way contains a 20 MB
unredistributable library inside
`Chromium.app/Contents/Frameworks/…/Libraries/WidevineCdm/`. Do not ship it —
`core/build/build.sh` says so when such a build finishes.

## Acceptable use

Fury is a privacy and multi-account management tool, built for QA, ad
verification, market research, scraping within terms, and running several
legitimate business accounts. Using it for fraud, credential stuffing, phishing
or evading law enforcement is not supported and not welcome in this issue
tracker.

Fury is an independent project, not affiliated with or endorsed by Google, and
carries no Chrome or Google branding. Chromium is used under its own licence.
The User-Agent declares Chrome because sites branch on it and a browser that says
anything else is distinguishable in one line — which is the whole point of the
exercise, and is what every Chromium fork does.

## Support

Everything here is free, for everyone, and stays that way. No seats, no
per-profile pricing, no paid tier held back, no telemetry. That is a decision
rather than a stage: the licence is AGPL and every measurement is published
precisely so nobody has to take anyone's word for what the browser does.

There is no company behind this and nothing is sold. What it costs is ordinary
and dull — a code-signing certificate so the download does not fight Gatekeeper,
and a machine to rebuild the core against every Chromium release
([docs/11](docs/11-budget.md) itemises it). If the project is useful to you and
you want to put something toward that:

**USDT (TRC20)** — `TBdbQDuUKHf14gvuyjSWevuL6FMS19ABzG`

Nothing is gated behind it, now or later. And a persona measured on your own
machine is still worth more than money — see [Contributing](#contributing).

## Author

Bogdan Shapovalov — [@shapovalovbogdan](https://t.me/shapovalovbogdan) on Telegram.

Questions about the measurements, a site that caught a profile, or a persona
from your own machine are all welcome there or in the issue tracker.
