# Fury

An open-source, self-hostable anti-detect browser for Windows and macOS, with first-class
team collaboration: organizations, projects, browser profiles, and granular access control.

> **Status: pre-alpha.** Design documents and skeleton only. Nothing here is production-ready yet.

## What it is

Most anti-detect browsers are closed-source SaaS: you upload your cookies and account
credentials to someone else's server and trust them. Fury inverts that:

- **Engine-level spoofing.** Fingerprint protection is compiled into a patched Chromium fork
  (C++ patches in Blink / V8 / BoringSSL / `//net`), not injected as JavaScript. There is no
  `Function.prototype.toString` tell, no leak through `Worker` / `iframe` / `ServiceWorker`,
  and the network-layer fingerprint (JA3/JA4, HTTP/2 SETTINGS) matches the claimed browser.
- **Self-hosted backend.** Your team's profiles, cookies and credentials live on infrastructure
  you control. Envelope encryption means the server stores ciphertext.
- **Real team model.** Organization → Project → Profile, with a permission matrix that supports
  the case every agency needs: *an operator can launch a profile but cannot see the password,
  cannot see the proxy credentials, and cannot export the cookies.*
- **Free and open source.** No seats, no per-profile pricing, no telemetry.

## Repository layout

| Path | What it is | Language | State |
|---|---|---|---|
| `core/` | Chromium fork: patch series, build scripts, GN args | C++ / shell | Scripts and series written, **no patches yet** |
| `agent/` | Local daemon: profile launcher, proxy relay, automation API | Rust | Relay and launcher args implemented |
| `server/` | Self-hosted backend: orgs, projects, RBAC, profile sync | Rust (axum + Postgres) | Schema and permission resolution |
| `shared-rs/` | Types shared by agent and server | Rust | Permission model, fingerprint spec |
| `shared/` | Cross-language schemas | JSON Schema | Persona schema |
| `tools/detect-suite/` | The measuring instrument: fingerprint probe, differ, release gate | JS / Rust / Python | **Working** |
| `desktop/` | Desktop app (profile manager UI) | Tauri 2 + React | Not started |
| `docs/` | Design documents (currently Russian, translation planned) | Markdown | Complete |

## Documentation

| Doc | Contents |
|---|---|
| [01 — Architecture](docs/01-architecture.md) | Components, processes, data flow, tech choices |
| [02 — Fingerprint surface](docs/02-fingerprint-surface.md) | Every vector, where it lives, where to patch it |
| [03 — Chromium fork](docs/03-chromium-fork.md) | Building, patch management, the 4-week rebase treadmill |
| [04 — Data model & RBAC](docs/04-data-model-rbac.md) | Schema, roles, permission matrix |
| [05 — Proxy & networking](docs/05-proxy-networking.md) | Relay design, DNS, WebRTC, leak prevention |
| [06 — Profile sync](docs/06-profile-sync.md) | Bundle format, encryption, locking, conflicts |
| [07 — Detection baseline](docs/07-detection-baseline.md) | Test harness and measurable pass criteria |
| [08 — Competitors](docs/08-competitors.md) | Feature matrix and where the gaps are |
| [09 — Roadmap](docs/09-roadmap.md) | Phased plan with exit criteria |
| [10 — Legal & licensing](docs/10-legal-licensing.md) | Chromium BSD, Widevine, branding, code signing |
| [11 — Budget](docs/11-budget.md) | What costs money, what does not, and what the real cost is |
| [12 — UX reference](docs/12-ui-reference.md) | What to copy from AdsPower, and the three places to beat it |
| [detect-suite](tools/detect-suite/README.md) | How the measuring instrument works and why it has two modes |

## Quick start (development)

Nothing builds end-to-end yet — there are no Chromium patches, so there is no
browser to launch. The parts that stand alone today:

```bash
cargo test --workspace
```

Run a proxy relay by itself and point any browser at the port it prints:

```bash
cargo run -p fury-agent -- relay socks5://user:pass@proxy.example:1080
```

Check a fingerprint config for internal contradictions:

```bash
cargo run -p fury-agent -- check-fingerprint -
```

Capture a fingerprint baseline from a real browser, no clicking required:

```bash
tools/detect-suite/capture-chrome.sh
```

Gate a captured dump — exits non-zero on failure, so it drops into CI:

```bash
cargo run -p fury-detect -- gate tools/detect-suite/baselines/candidate.json
```

Bring up the self-hosted backend:

```bash
cp .env.example .env && docker compose up -d
```

See [docs/09-roadmap.md](docs/09-roadmap.md) for what comes next.

## Licensing

- `core/patches/` — derived from Chromium, **BSD-3-Clause** (upstream terms preserved).
- `server/`, `agent/`, `desktop/` — **AGPL-3.0-or-later**.

Rationale and alternatives in [docs/10-legal-licensing.md](docs/10-legal-licensing.md).

## Acceptable use

Fury is a privacy and multi-account management tool. It is built for QA, ad verification,
market research, web scraping within terms, and managing multiple legitimate business accounts.
Using it for fraud, credential stuffing, phishing, or evading law enforcement is not supported
and not welcome in this project's issue tracker.
