# 26 — Creating Automations & Preserving Anonymity

Automations drive Fury profiles through the [Local API](../examples/README.md), the
built-in MCP server (`fury-agent mcp`), Playwright, Puppeteer, or standard-library
CDP clients. Proposed in [issue 30](https://github.com/furyteamtop/fury-antidetect-browser/issues/30).

Official entries in [`examples/`](../examples/) (`api.sh`, `playwright_example.py`,
`puppeteer_example.mjs`, `rotate.py`) form the catalogue baseline. Built-in and community
automations reside in [`automations/`](../automations/) (such as [`automations/interactive-warmup/`](../automations/interactive-warmup/)).

## Background & Architectural Decisions

This automation architecture and the community runner originate from the open-source
discussion in [Issue 30](https://github.com/furyteamtop/fury-antidetect-browser/issues/30).
Issue 30 established the foundation: the automation catalogue layout, manifest schema
(`automation.json`), and strict alignment with [`ACCEPTABLE_USE.md`](../ACCEPTABLE_USE.md).
Building upon that foundation, this document defines the operational, anti-detection,
and execution standards required to ensure community automations operate without
compromising Fury's core spoofing or leaking host system attributes:

1. **Foundational Examples Preserved & Acceptable Use Boundary:**
   Issue 30 establishes that the existing repository entries—[`api.sh`](../examples/api.sh),
   [`playwright_example.py`](../examples/playwright_example.py),
   [`puppeteer_example.mjs`](../examples/puppeteer_example.mjs), and
   [`rotate.py`](../examples/rotate.py)—are the foundational first entries of the automation
   catalogue. The automation framework builds on top of this foundation while anchoring
   strictly to [`ACCEPTABLE_USE.md`](../ACCEPTABLE_USE.md). Automations must only simulate
   legitimate human actions: reading articles, natural dwells, link exploration, and clean
   session management. Automations targeting credential stuffing, mass account creation,
   fake reviews, or platform fraud are barred by project policy and closed without review.

2. **Zero Host Network Calls (Preventing OpenSSL TLS Leaks):**
   Auxiliary network requests from Python/Node scripts—even when routed through the local
   relay port via HTTP CONNECT—leak the host machine's Python OpenSSL/LibreSSL TLS handshake.
   The resulting TLS ClientHello lacks BoringSSL GREASE, presents OpenSSL cipher suites,
   and yields a Python/OpenSSL JA3/JA4 fingerprint that contradicts Chromium's User-Agent,
   causing immediate anti-bot bans (`docs/02-fingerprint-surface.md`).
   Therefore, **all network traffic must originate inside the Chromium core process** via
   CDP navigation or in-page DOM operations.

3. **Viewport Geometry & Upstream Real-Screen Policy (Issues #45 & #47):**
   As documented in upstream issues [#45](https://github.com/furyteamtop/fury-antidetect-browser/issues/45)
   and [#47](https://github.com/furyteamtop/fury-antidetect-browser/issues/47), Fury's desktop profiles
   currently implement an explicit real-screen policy (`0020-screen-window.patch`), where
   profiles without an explicit screen override expose the physical host monitor. Until this
   is resolved upstream in the C++ core and launcher, automations cannot force screen
   metrics. Automations must adapt dynamically by reading `window.innerWidth` and
   `window.innerHeight` and strictly clamping cursor dynamics to the active visible viewport.

4. **Sanitized Host Environment Isolation:**
   To prevent host proxy configs, developer paths, and ambient user accounts from leaking
   into automation child runtimes, the execution runner (`desktop/src-tauri/src/automations.rs`)
   enforces `cmd.env_clear()`. Only explicitly sanitized variables (`PATH`, `FURY_API_PORT`,
   `FURY_PROFILE_IDS`, `FURY_HOME`, `PYTHONPATH`, `PYTHONUNBUFFERED`, `PYTHONIOENCODING`,
   `TMPDIR`) are passed to child processes.

## Principles

1. **Spoofing lives in the core, not the script.**
   All fingerprint attributes (CPU concurrency, memory, GPU strings, Skia canvas
   noise, fonts, audio buffers, timezone, and Client Hints) are established in
   Chromium's C++ core ([`core/patches/series`](../core/patches/series)) before an
   automation attaches. An automation script does not inject spoofing shims.

2. **Zero host network leaks (No auxiliary script requests).**
   All browser traffic routes through Fury's local proxy relay (`127.0.0.1:<relay_port>`),
   which enforces DNS isolation, WebRTC IP policy, and proxy credentials. Automation
   scripts must **never** make direct or proxy-relayed HTTP/HTTPS requests using host
   Python (`urllib`, `requests`) or Node (`fetch`, `http`) libraries. Auxiliary requests
   made outside Chromium perform TLS handshakes using the host's OpenSSL/LibreSSL stack,
   revealing non-Chromium JA3/JA4 fingerprints and header orderings.
   **100% of network traffic must originate from inside the Chromium core.**

3. **Dynamic viewport bounds & upstream screen alignment.**
   Because desktop profiles without explicit screen overrides inherit host monitor bounds
   under Fury's current core policy (Issues #45 & #47), automations must never assume
   arbitrary hardcoded resolutions (such as 1280×800) or assume persona metadata matches
   `window.screen`. Automations must query `window.innerWidth` and `window.innerHeight`
   dynamically via CDP and strictly clamp all mouse paths, Bézier control points, and
   clicks within the document's active visible bounds.

4. **Sanitized host environment (`env_clear`).**
   Automation runners must completely isolate the child execution environment using
   `cmd.env_clear()`. Ambient host variables (`USER`, `LOGNAME`, `SHELL`, `LANG`,
   `SSH_AUTH_SOCK`, host proxy configurations) must never leak into child processes.
   Runners supply only sanitized parameters (`PATH`, `FURY_API_PORT`, `FURY_PROFILE_IDS`,
   `FURY_HOME`, `PYTHONPATH`, `PYTHONUNBUFFERED`, `PYTHONIOENCODING`, `TMPDIR`).

5. **Clean CDP lifecycle.**
   Never call `browser.close()` over CDP. Abruptly terminating Chromium prevents
   SQLite from flushing cookies to disk and interrupts the agent's bundle encryption
   ([`agent/src/ipc.rs`](../agent/src/ipc.rs)). Exit the driver block or call
   `client.close()` / `disconnect()`, allowing `with running(fury, profile_id)` to manage
   profile shutdown cleanly via `POST /profiles/stop`.

6. **Sequential execution.**
   Concurrent automation runs hitting the same domain across multiple profiles
   create synchronized timing fingerprints. Automations should rotate through
   profiles sequentially with irregular dwell intervals ([`examples/rotate.py`](../examples/rotate.py)).

## Driver Selection & Trade-Offs

| Driver | Dependencies | `Runtime.enable` Timing Tell | Primary Use Case |
|---|---|---|---|
| **Playwright via CDP** | `playwright` | 2.7× delay on `console.debug` (auto-enables `Runtime`) | Complex SPAs, form fills, shadow DOM traversal. |
| **Puppeteer via CDP** | `puppeteer-core` | 2.7× delay on `console.debug` (auto-enables `Runtime`) | Node.js automation pipelines. |
| **Pure CDP Client** | Standard library | None (`Runtime.evaluate` without domain enablement) | Lightweight warmup, link discovery, zero pip dependencies. |

*Note on timing:* As measured on Fury 150 ([`tools/detect-suite/cdp-timing.py`](../tools/detect-suite/cdp-timing.py)),
attaching to CDP introduces zero overhead (2.8 µs). Activating `Runtime.enable`
adds a fixed ~5 µs overhead per `console.debug` call (8.0–25.5 µs vs 2.8 µs baseline).
Real Chrome behaves identically. Pure CDP scripts that execute `Runtime.evaluate`
without calling `Runtime.enable` keep debugging domains uninstrumented.

## Implementation Patterns

### Pattern 1: Playwright via CDP

Always attach to the running profile via `connect_over_cdp` and reuse the existing
default context (`contexts[0]`):

```python
from fury import Fury, running
from playwright.sync_api import sync_playwright

with running(Fury(), profile_id, cdp=True) as session:
    with sync_playwright() as pw:
        browser = pw.chromium.connect_over_cdp(session["ws_endpoint"])
        context = browser.contexts[0]
        page = context.pages[0] if context.pages else context.new_page()

        page.goto("https://example.com", wait_until="domcontentloaded")
        # Exiting the block disconnects cleanly without closing the browser
```

### Pattern 2: Pure Standard-Library CDP

For tasks requiring zero external packages and minimal timing overhead, connect
directly via WebSocket RFC 6455 ([`automations/interactive-warmup/cdp.py`](../automations/interactive-warmup/cdp.py))
and dispatch trusted CDP input events (`Input.dispatchMouseEvent`).

Note that `session["ws_endpoint"]` returns the browser-level WebSocket endpoint.
Pure CDP clients querying the active tab resolve the page-level target via `/json/list`:

```python
import json
import urllib.request
from cdp import CDPClient
from fury import Fury, running

with running(Fury(), profile_id, cdp=True) as session:
    # Resolve page WebSocket from the CDP port
    cdp_port = int(session["ws_endpoint"].split(":")[2].split("/")[0])
    with urllib.request.urlopen(f"http://127.0.0.1:{cdp_port}/json/list") as resp:
        targets = json.load(resp)
    page_ws = next(t["webSocketDebuggerUrl"] for t in targets if t.get("type") == "page")

    client = CDPClient(page_ws)
    try:
        # Isolated evaluate without Runtime.enable
        client.eval("location.href = 'https://example.com'")
    finally:
        client.close()
```

### Pattern 3: In-Browser Navigation & In-Page Fetch (Zero Host Network Calls)

When dynamic resources, content, or API data must be fetched, dispatch the request
**inside the browser realm** so that traffic flows through Chromium's BoringSSL and
proxy relay kill-switch:

```python
# Perform all network interactions via CDP in-page fetch rather than host Python urllib
content = client.eval("""(async () => {
    const resp = await fetch('https://example.com/data.json');
    return await resp.text();
})()""")
```

## Structure & Submission

Place automations in `./automations/<name>/`:

```
automations/interactive-warmup/
├── automation.json     # Metadata manifest (Issue 30 standard)
├── warm_up.py          # Entry script accepting <profile-id>
├── cdp.py              # Pure RFC 6455 WebSocket CDP client
├── human.py            # Bézier curves, dynamic viewport clamping, trusted input
├── fury.py             # Local API client and running context manager
└── README.md           # Documentation, requirements, tested platforms
```

Automations placed in `./automations/<name>/` or imported into the Fury application
data directory (`~/Library/Application Support/Fury/automations/<name>/` on macOS,
`~/.local/share/fury/automations/<name>/` on Linux, or `%APPDATA%\Fury\automations\<name>\` on Windows)
are automatically discovered and surfaced in Fury Desktop under the **Automations** tab.
Users can launch them across selected profiles with one click and monitor live log output.

### Packaging as a `.fury` File

Automations can be shared as a single `.fury` package file:

1. **Format:** A `.fury` file is simply a standard `.zip` archive containing `automation.json` and the script files at its root, renamed with the `.fury` extension.
2. **From Desktop GUI:** Open the **Automations** tab in Fury Desktop and click **Export** on any listed automation.
3. **From Command Line:**
   ```bash
   cd my-automation/
   zip -r ../my-automation.fury . -x "*.pyc" -x ".*"
   ```
4. **Importing:** In Fury Desktop, click **Import Package** and select the `.fury` file. Fury safely verifies the manifest and extracts it into the local automations directory.

### Review Criteria

Pull requests must satisfy the project's verification standards:
* **Strict ACCEPTABLE_USE.md compliance:** Does something a real person could do. No credential stuffing, mass botting, fake reviews, or fraud.
* **Token from file:** Read from `api-token` ([`examples/fury.py`](../examples/fury.py)), never passed as a CLI flag.
* **Profiles created in app:** The API deliberately lacks a profile creation endpoint.
* **No browser leaks on failure:** Wrapped in `with running(...)`.
* **Clean CDP disconnect:** No `browser.close()` calls over CDP.
* **Zero host network leaks:** No auxiliary Python/Node network calls; 100% of web traffic originates in Chromium.
* **Sanitized runner environment:** Runner isolates the child process with `cmd.env_clear()`.
* **Dynamic viewport bounds:** Cursor trajectories clamped dynamically within `window.innerWidth`/`innerHeight` without arbitrary hardcoded bounds.
* **Trusted input events:** CDP `Input.dispatchMouseEvent`, zero synthetic `element.click()`.
* **Verified:** Pull request records the Fury release, operating system, and verification results (`core/verify/verify-0300.py`).
