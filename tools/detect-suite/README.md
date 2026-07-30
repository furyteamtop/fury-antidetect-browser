# detect-suite

The measuring instrument. Everything else in Fury is judged by what this reports.

Built **before** the first Chromium patch on purpose: writing patches first and
checking later is how a team spends six months on code that does not work.
See [docs/07](../../docs/07-detection-baseline.md) for the release gate this feeds.

## Parts

| File | What it does |
|---|---|
| `probe.js` | Dumps ~250 fingerprint values, then re-reads them from a Worker and four kinds of iframe and reports where the answers disagree |
| `probe.html` | Open in any browser under test. Runs the probe, shows pass/fail on the critical checks, saves the dump |
| `collector.py` | Serves `probe.html` and accepts dumps POSTed back into `baselines/` |
| `capture-chrome.sh` | Captures a baseline with no clicking: launches a browser on a throwaway profile pointed at `probe.html?auto=<name>` |
| `src/main.rs` | `fury-detect` — diff two dumps, or gate a single one |
| `src/classify.rs` | Decides whether a given difference matters, and in which mode |

## The one idea worth understanding

A naive comparison of two fingerprint dumps reports 400 differences and tells you
nothing. The question is always *what kind* of difference it is:

- **Value** differences — canvas hash, GPU string, user agent, font list.
  Between real Chrome and Fury these **should** differ. That is the spoofing working.
- **Behavioural** differences — an API that exists in one and not the other, a
  getter that stopped being native, a codec that stopped being supported, a
  stability flag that flipped, cross-context consistency breaking.
  These must **never** differ, because they let a site tell the two apart by
  *what the browser can do* rather than by what it claims.

Hence two modes:

```bash
# Same browser, two runs. Nothing may differ — instability is worse than no
# spoofing, because a site that reads a value twice sees one identity on
# changing hardware.
fury-detect diff --mode identity run-a.json run-b.json
```

```bash
# Real Chrome vs Fury. Values may differ, behaviour may not.
fury-detect diff --mode spoof baselines/chrome-151-macos-arm.json candidate.json
```

## Capturing a baseline

Automatic, for anything launchable from the command line:

```bash
tools/detect-suite/capture-chrome.sh
```

```bash
tools/detect-suite/capture-chrome.sh camoufox-146 /path/to/camoufox
```

It starts the collector if needed, launches the browser on a **throwaway
profile** — a reference baseline must describe a clean browser, not one shaped by
whatever extensions the operator installed — waits for the dump, and closes it.

Manual, for browsers you cannot launch with flags (AdsPower profiles, Fury
profiles): start the collector and open the auto URL inside the profile.

```bash
python3 tools/detect-suite/collector.py
```

```
http://127.0.0.1:8731/probe.html?auto=adspower
```

`?auto=<name>` runs the probe and stores it with no clicking. Use `127.0.0.1`
rather than `localhost`: see the findings below.

Serve it over http, not `file://`: workers and iframes get an opaque origin under
`file://` and the cross-context comparison — the whole point — comes out empty.

Committed reference baselines are named `chrome-<major>-<os>-<arch>.json` and must
come from a clean machine with a real Chrome install. Everything else in
`baselines/` is gitignored: a dump contains the real hardware fingerprint of
whoever captured it.

## Gating a single dump

Some failures need no baseline because they are internal contradictions:

```bash
fury-detect gate candidate.json
```

Exits non-zero on failure, so it drops straight into CI.

Checks: cross-context consistency · canvas/WebGL/audio stability across calls ·
getters still native · H.264 and AAC present · Widevine accepted · Permissions
API agrees with `Notification.permission` · no ChromeDriver traces ·
`navigator.webdriver === false` · scrollbar width matches the claimed OS · WebGPU
describes the same GPU as WebGL · timezone reaches workers.

## Findings from building it

Four things this tool established, each of which changed something:

**`document.fonts.check()` is not a font-detection method.** Verified: Chrome
returns `true` for arbitrary family names, including
`"ThisFontDoesNotExistAnywhere12345"`. It answers "can this be rendered", and
fallback makes the answer always yes. Real font detection is measurement — render
with the candidate font and a known fallback, compare widths — which is why
patch `0050` has to filter at the font-fallback layer and not at the enumeration
API. A list-only filter is defeated in four lines of JS.

**`null` from a context is an absence, not a value.** A Worker has no `screen`
and no `navigator.webdriver`. Reporting those as cross-context disagreements
produced false positives that buried the real ones, so absences are now tracked
separately in `crossContext.absences` — and a *new* absence appearing there is
itself a finding, because it means a patch removed an API it should have spoofed.

**The two permission APIs use different words for the same state.**
`navigator.permissions.query({name:'notifications'}).state` returns
`granted | denied | prompt`, while `Notification.permission` returns
`granted | denied | default`. So `prompt` and `default` are the *same* state.
Comparing the strings directly marks **real Chrome as inconsistent** — which is
exactly what happened the first time the gate ran against a genuine Chrome 150
baseline. This is the whole argument for building the instrument before the
patches: an uncalibrated gate reports failures that are its own.

**A diagnostic channel must never be selective.** `collector.py` originally
logged only successful POSTs, which also hid every `send_error()` path. When a
dump failed to arrive there was no way to tell "the browser never asked" from
"the server refused it", and a real capture attempt was lost to that blind spot.
It now logs every request. Relatedly, the capture URL uses `127.0.0.1` and not
`localhost`: on macOS `localhost` may resolve to `::1` first, and a v4-only
collector then never sees the request at all.

## Not built yet

Probing `SharedWorker`, `ServiceWorker`, `AudioWorklet` and a genuinely
cross-origin OOPIF. The current cross-context comparison covers the main frame, a
Worker and four kinds of iframe — enough to catch the common leak, not yet the
full surface from [docs/02](../../docs/02-fingerprint-surface.md) layer 3.

CDP-driven capture. `capture-chrome.sh` covers everything launchable with flags,
which is all that CI needs; CDP would additionally reach browsers that only
expose a debugging port.
