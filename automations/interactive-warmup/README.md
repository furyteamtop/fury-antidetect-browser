# Interactive Warmup

A zero-dependency profile warmup automation for **Fury**.

Drives profiles through the Fury Local API and native Chrome DevTools Protocol (CDP), visiting seed websites with human-realistic browsing dynamics:
- Smooth cubic Bézier mouse movement with micro-jitters strictly clamped to the profile persona's viewport.
- Natural cosine-eased wheel scrolling (trusted `mouseWheel` events).
- Automatic consent banner dismissal via trusted CDP clicks (zero synthetic `element.click()`).
- Article and headline exploration directly on target pages.
- Sequential profile rotation with irregular dwell gaps to prevent cross-profile clustering.

Uses **100% Python standard library** (`urllib`, `socket`, `struct`, `json`, `time`, `math`, `random`). Zero external pip packages needed.

---

## Anti-Detection & Architecture Guarantees

As documented in [26 — Creating Automations & Preserving Anonymity](../../docs/26-creating-automations.md):
- **Dynamic Viewport Clamping:** Viewport bounds are queried dynamically from the active document (`window.innerWidth`, `window.innerHeight`), strictly clamping cursor dynamics within visible bounds under Fury's desktop monitor policies.
- **Bézier Cursor Dynamics:** Mouse trajectories are generated using quadratic Bézier curves with micro-tremor and clamped within the active document's viewport.
- **Trusted CDP Events:** Input is dispatched via native CDP `Input.dispatchMouseEvent` (`mousePressed`, `mouseReleased`, `mouseWheel`), avoiding synthetic `element.click()` injection tells.
- **Zero Host Network Calls:** 100% of network traffic originates strictly within Chromium via CDP. The script makes zero auxiliary HTTP/HTTPS requests from host Python, preserving BoringSSL TLS parity.
- **Environment Isolation:** Child process runs with an isolated execution environment (`cmd.env_clear()`), stripping ambient host variables (`USER`, `HOME`, `SHELL`, `LANG`) and host proxy configurations.

---

## Requirements

- Python 3.9+
- Fury running with Local API enabled: `FURY_API_PORT=35000 fury-agent serve`

---

## Usage

### Run on a single profile:
```bash
python3 warm_up.py <profile-id>
```

### Run on multiple profiles sequentially:
```bash
python3 warm_up.py <profile-id-1> <profile-id-2> --sites 4 --min-dwell 10 --max-dwell 20
```

### Run with custom seed targets:
```bash
python3 warm_up.py <profile-id> --custom-urls https://news.ycombinator.com https://en.wikipedia.org
```

---

## Tested On

- Fury 0.6.9, macOS 15+ & Linux x86_64
