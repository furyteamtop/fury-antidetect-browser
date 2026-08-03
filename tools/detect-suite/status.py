#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Generate the status page from the captures in git.

    ./status.py                 write status.html
    ./status.py --check         fail if status.html is out of date

The project's whole claim is that every number has a measurement behind it. That
claim is worth nothing if reading the measurements means cloning the repo,
building a browser and running a probe — so this turns the captures that are
already tracked into one page anybody can open.

Generated from tracked, redacted captures ONLY. A capture carries the public
address the proxy exits from, which is why the raw ones are gitignored; this
reads the redacted ones, which have RFC 5737 documentation addresses where the
real ones were. If a source file is missing, the section says so rather than
being quietly left out — a page that silently omits a failing check is worse
than no page.
"""

import argparse
import html
import json
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
BASELINES = HERE / "baselines"
OUT = HERE / "status.html"

# The eight contexts a serious checker compares. A value that differs between
# any two of them is a spoof that reached one place and not another, which is
# caught in one line of JS.
CONTEXTS = [
    ("main", "main frame"),
    ("worker", "dedicated Worker"),
    ("sharedWorker", "SharedWorker"),
    ("serviceWorker", "ServiceWorker"),
    ("audioWorklet", "AudioWorklet"),
    ("iframe:same-origin", "iframe, same origin"),
    ("iframe:about:blank", "iframe, about:blank"),
    ("iframe:srcdoc", "iframe, srcdoc"),
]

# What the comparison is actually about. Not every field in a capture — the
# interesting ones, in the order somebody would want to read them.
COMPARED = [
    ("navigator.userAgent", "user agent"),
    ("navigator.platform", "platform"),
    ("navigator.hardwareConcurrency", "cores"),
    ("navigator.deviceMemory", "memory (GB)"),
    ("screen.width", "screen width"),
    ("screen.height", "screen height"),
    ("locale.timezone", "timezone"),
    ("webgl.webgl1.unmasked.vendor", "WebGL vendor"),
    ("webgl.webgl1.unmasked.renderer", "WebGL renderer"),
    ("fonts.countByMeasurement", "fonts detected"),
    ("canvas2d.stableAcrossCalls", "canvas stable"),
]


def dig(obj, path):
    for part in path.split("."):
        if not isinstance(obj, dict) or part not in obj:
            return None
        obj = obj[part]
    return obj


def load(name):
    try:
        return json.loads((BASELINES / f"{name}.json").read_text())
    except FileNotFoundError:
        return None


def gate(name):
    """Run the real gate rather than reimplementing it.

    Reimplementing thirteen checks in a page generator would produce a page
    that agrees with itself and not with the tool anybody runs. If the binary
    is not built, the section says so.
    """
    path = BASELINES / f"{name}.json"
    if not path.exists():
        return None, f"no {path.name}"
    try:
        proc = subprocess.run(
            ["cargo", "run", "-q", "-p", "fury-detect", "--", "gate", str(path)],
            cwd=HERE.parent.parent,
            capture_output=True,
            text=True,
            timeout=600,
        )
    except (FileNotFoundError, subprocess.TimeoutExpired) as e:
        return None, f"could not run the gate: {e}"

    rows = []
    for line in proc.stdout.splitlines():
        # The check lines are indented; the summary line is not, and it begins
        # with the same word ("PASS — all gate checks satisfied"), which is how
        # a thirteen-check gate first rendered as fourteen rows.
        if not line.startswith("  "):
            continue
        line = line.strip()
        for mark in ("PASS ", "FAIL ", "skip "):
            if line.startswith(mark):
                rest = line[len(mark):].strip()
                # The tool pads the check name to a column; split on the run of
                # spaces it uses rather than on the first one, since check names
                # contain spaces and so do details.
                parts = rest.split("  ", 1)
                rows.append((mark.strip(), parts[0].strip(), parts[1].strip() if len(parts) > 1 else ""))
                break
    return rows, None


def e(x):
    return html.escape(str(x)) if x is not None else "—"


def build():
    fury = load("ctx-fury-redacted")
    chrome = load("ctx-chrome-redacted")
    gate_rows, gate_error = gate("gate-persona-redacted")

    try:
        commit = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=HERE, capture_output=True, text=True, timeout=30,
        ).stdout.strip()
    except Exception:
        commit = ""

    parts = [HEAD]

    # --- the headline ---------------------------------------------------
    if fury and chrome:
        f_dis = dig(fury, "crossContext.disagreementCount")
        c_dis = dig(chrome, "crossContext.disagreementCount")
        f_ctx = len(dig(fury, "crossContext.contextsProbed") or [])
        c_ctx = len(dig(chrome, "crossContext.contextsProbed") or [])
        verdict_class = "ok" if f_dis == 0 else "bad"
        parts.append(f"""
<section class="headline {verdict_class}">
  <p class="number">{e(f_dis)}</p>
  <p class="label">cross-context disagreements, across {e(f_ctx)} contexts</p>
  <p class="note">Real Chrome on the same machine: <strong>{e(c_dis)}</strong> across {e(c_ctx)}.
     This is the number that matters. A spoof that reaches the main frame and
     forgets <code>new Worker()</code> is caught in one line of JavaScript, and
     the only acceptable answer is the one Chrome gives.</p>
</section>""")
    else:
        parts.append(section_missing("cross-context", "ctx-fury-redacted.json / ctx-chrome-redacted.json"))

    # --- context table --------------------------------------------------
    if fury:
        probed = set(dig(fury, "crossContext.contextsProbed") or [])
        rows = "".join(
            f"<tr><td>{e(label)}</td><td class=\"{'yes' if key in probed else 'no'}\">"
            f"{'probed' if key in probed else 'not reached'}</td></tr>"
            for key, label in CONTEXTS
        )
        # A field missing from a context is not a disagreement, and the two are
        # worth keeping apart. navigator.userAgent does not exist inside an
        # AudioWorklet and navigator.webdriver does not exist in any worker —
        # in Chrome either. What would be alarming is a field present in Fury
        # and absent in Chrome, or the reverse, so the two lists are compared
        # rather than only Fury's being printed.
        f_abs = dig(fury, "crossContext.absences") or []
        c_abs = dig(chrome, "crossContext.absences") if chrome else None
        note = ""
        if f_abs:
            lines = "".join(
                f"<li><code>{e(a.get('field'))}</code> is not defined in "
                f"{e(', '.join(a.get('absentIn') or []))}</li>"
                for a in f_abs
            )
            if c_abs == f_abs:
                verdict = ("Chrome's list is identical, which is the answer that matters: "
                           "these are absences of the platform, not of the spoofing.")
            elif c_abs is None:
                verdict = "No Chrome capture to compare against."
            else:
                verdict = ("<strong>Chrome's list differs</strong>, which is a difference a "
                           "page can see — a field defined in one browser and not the other.")
            note = (f'<p class="note">Fields that do not exist in every context:</p>'
                    f'<ul class="note">{lines}</ul><p class="note">{verdict}</p>')
        parts.append(f"""
<section>
  <h2>The eight contexts</h2>
  <table><tbody>{rows}</tbody></table>
  {note}
</section>""")

    # --- gate -----------------------------------------------------------
    if gate_rows:
        passed = sum(1 for m, _, _ in gate_rows if m == "PASS")
        failed = sum(1 for m, _, _ in gate_rows if m == "FAIL")
        # Parsing another program's output is a place to be wrong quietly. The
        # gate has thirteen checks; a count that is not thirteen means this
        # parser has drifted from it and the page would be confidently wrong.
        if len(gate_rows) != 13:
            raise SystemExit(
                f"parsed {len(gate_rows)} gate rows, expected 13 — "
                f"status.py has drifted from cmd_gate in tools/detect-suite/src/main.rs"
            )
        rows = "".join(
            f'<tr class="{m.lower()}"><td>{e(m)}</td><td>{e(what)}</td><td>{e(detail)}</td></tr>'
            for m, what, detail in gate_rows
        )
        parts.append(f"""
<section>
  <h2>Release gate <span class="tally">{passed} passed, {failed} failed</span></h2>
  <p class="note">Run it yourself: <code>cargo run -p fury-detect -- gate &lt;capture.json&gt;</code>.
     This table is that command's output, not a second implementation of it.</p>
  <table><tbody>{rows}</tbody></table>
</section>""")
    else:
        parts.append(section_missing("release gate", gate_error or "gate-persona-redacted.json"))

    # --- side by side ---------------------------------------------------
    if fury and chrome:
        rows = ""
        for path, label in COMPARED:
            f_val, c_val = dig(fury, path), dig(chrome, path)
            # Both absent means the path is wrong, not that the browsers agree.
            # This rendered "WebGL vendor — — same" until it was looked at,
            # which is the most confident way to say nothing.
            if f_val is None and c_val is None:
                raise SystemExit(f"{path} resolves in neither capture — fix the path in COMPARED")
            same = "same" if f_val == c_val else "differs"
            rows += (f"<tr><td>{e(label)}</td><td>{e(f_val)}</td><td>{e(c_val)}</td>"
                     f'<td class="{same}">{same}</td></tr>')
        parts.append(f"""
<section>
  <h2>Fury and Chrome, side by side</h2>
  <p class="note">Both captures were taken on the same machine. <em>differs</em> is
     the expected answer for a spoofed profile and <em>same</em> is the expected
     answer for everything the persona does not claim — the point is that each
     one is deliberate.</p>
  <table>
    <thead><tr><th></th><th>Fury</th><th>Chrome 150</th><th></th></tr></thead>
    <tbody>{rows}</tbody>
  </table>
</section>""")

    parts.append(f"""
<footer>
  <p>Generated by <code>tools/detect-suite/status.py</code> from the redacted
     captures tracked in the repository{f", at commit <code>{e(commit)}</code>" if commit else ""}.
     Addresses in those captures are RFC 5737 documentation addresses: a raw
     capture carries the public address the profile's proxy exits from, so the
     raw ones are not committed.</p>
  <p>Nothing here is taken on trust. <code>tools/detect-suite/probe.html</code>
     produces a capture from any browser, and every number above comes from
     one.</p>
</footer>
</main></body></html>""")

    return "\n".join(parts)


def section_missing(what, why):
    return f"""
<section class="missing">
  <h2>{html.escape(what)}</h2>
  <p>Not generated: {html.escape(why)}.</p>
  <p class="note">Said rather than omitted. A status page that quietly drops the
     section it could not produce reads as though everything passed.</p>
</section>"""


HEAD = """<!doctype html>
<meta charset="utf-8">
<title>Fury — measured</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  :root { color-scheme: light dark; --line: #8883; --ok: #1a7f37; --bad: #c1121f; }
  body { font: 16px/1.55 ui-sans-serif, system-ui, -apple-system, sans-serif;
         max-width: 54rem; margin: 0 auto; padding: 2rem 1.25rem 4rem; }
  h1 { font-size: 1.6rem; margin-bottom: .25rem; }
  h2 { font-size: 1.1rem; margin: 2.5rem 0 .5rem; }
  .sub { opacity: .75; margin-top: 0; }
  .note { opacity: .8; font-size: .9rem; }
  section.headline { border: 1px solid var(--line); border-radius: .6rem;
                     padding: 1.25rem 1.5rem; margin-top: 2rem; }
  .headline .number { font-size: 3.4rem; line-height: 1; margin: 0; font-weight: 650; }
  .headline.ok .number { color: var(--ok); }
  .headline.bad .number { color: var(--bad); }
  .headline .label { margin: .35rem 0 1rem; font-size: 1.05rem; }
  table { border-collapse: collapse; width: 100%; font-size: .92rem; }
  th, td { text-align: left; padding: .4rem .6rem; border-bottom: 1px solid var(--line); }
  th { font-weight: 600; opacity: .7; }
  td:first-child { white-space: nowrap; }
  tr.pass td:first-child { color: var(--ok); font-weight: 600; }
  tr.fail td:first-child { color: var(--bad); font-weight: 600; }
  tr.skip td:first-child { opacity: .55; }
  .yes { color: var(--ok); } .no, .missing h2 { color: var(--bad); }
  .differs { opacity: .75; } .same { opacity: .55; }
  .tally { font-weight: 400; font-size: .85rem; opacity: .7; }
  code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: .9em; }
  footer { margin-top: 3.5rem; padding-top: 1rem; border-top: 1px solid var(--line);
           font-size: .85rem; opacity: .8; }
</style>
<body><main>
<h1>Fury, measured</h1>
<p class="sub">Every number on this page came from a browser that was running.
   None of them are claims.</p>"""


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--check", action="store_true",
                    help="exit non-zero if status.html is not what this would write")
    args = ap.parse_args()

    page = build()

    if args.check:
        current = OUT.read_text() if OUT.exists() else ""
        if current.strip() != page.strip():
            print(f"!! {OUT.name} is out of date — run tools/detect-suite/status.py", file=sys.stderr)
            return 1
        print(f"{OUT.name} is up to date")
        return 0

    OUT.write_text(page)
    print(f"wrote {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
