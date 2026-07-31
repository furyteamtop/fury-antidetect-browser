# Fury

A free, open-source anti-detect browser with real team collaboration. Own
Chromium fork, works standalone with no server, self-hostable when you need a
team. No seats, no per-profile pricing, no telemetry.

*[Русская версия](README.ru.md)*

> **Status: in development.** The core builds and spoofs; the agent launches
> profiles; the server and desktop shell work. What is *not* done is listed at
> the bottom, honestly. There are no releases yet.

## Why another one

Anti-detect browsers cost $30–150 a month per team and are closed source, so
nobody can check what they actually spoof. We measured the popular ones: in one,
the canvas readback was **byte-identical to a clean machine** — not spoofed at
all, while the interface listed it as protected
([docs/08](docs/08-competitors.md)).

This is the inverse. Every vector is documented against the Chromium file it
lives in, the patches are in the repo, and you can measure it yourself with the
bench that ships alongside.

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

**Hosted** is planned, but gated behind client-side bundle encryption. The
condition is not negotiable: the server must hold data it cannot itself read. It
does not open before that works.

## What works today

A profile using a Windows 11 / RTX 4060 persona, launched on an Apple Silicon
MacBook, reports:

```
navigator.platform      Win32
userAgent               Windows NT 10.0; Win64; x64 … Chrome/150.0.0.0
WebGL renderer          ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 …)
screen                  1920×1080, availHeight 1032   ← the taskbar
Client Hints platform   Windows        brands: … Google Chrome/150
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

- **core** — Chromium 150 fork, [18 patches](core/patches/); spoofing is in C++,
  never injected JavaScript
- **agent** — the only component holding decrypted secrets: proxy relays,
  launching, the local automation API
- **server** — organizations, projects, permissions, locking. Deliberately dumb:
  it never generates a fingerprint and never decrypts a bundle
- **desktop** — Tauri rather than Electron: a 7 MB app, not 120 MB

Details in [docs/01](docs/01-architecture.md).

## Build

The core takes 2–3 hours and ~100 GB. Everything else takes minutes.

```bash
git clone https://github.com/fury-browser/fury && cd fury
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

## Check it yourself

Do not take any of the above on trust — measure it:

```bash
cd tools/detect-suite && python3 -m http.server 8791
```

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
| [10 — Legal & licensing](docs/10-legal-licensing.md) | Chromium BSD, Widevine, branding, code signing |
| [11 — Budget](docs/11-budget.md) | What costs money and what does not |
| [12 — UX reference](docs/12-ui-reference.md) | What to copy from AdsPower, and where to beat it |
| [13 — Self-hosting](docs/13-self-hosting.md) | Standing up a team server |

Documents are in Russian; translation is planned.

## Not done yet

| | |
|---|---|
| Creating and editing profiles in the UI | no — only over the agent socket |
| Entering proxies in the UI | no |
| Settings, Russian localisation | no |
| Exporting and importing projects | no |
| Bundle sync with the server | no |
| Client-side bundle encryption (vault) | no — this blocks hosted mode |
| Windows build | never compiled once |
| Code signing and notarisation | no; macOS will complain |

## Licensing

- `core/patches/` — derived from Chromium, **BSD-3-Clause** (upstream terms preserved)
- `server/`, `agent/`, `desktop/` — **AGPL-3.0-or-later**

AGPL is deliberate: anyone running a service on this code has to publish their
changes, which is what keeps "free" free. Reasoning in
[docs/10](docs/10-legal-licensing.md).

## Acceptable use

Fury is a privacy and multi-account management tool, built for QA, ad
verification, market research, scraping within terms, and running several
legitimate business accounts. Using it for fraud, credential stuffing, phishing
or evading law enforcement is not supported and not welcome in this issue
tracker.

Fury is an independent project, not affiliated with Google. Chromium is used
under its own licence.
