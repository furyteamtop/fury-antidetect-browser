#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Regenerate sw-probe.js from probe.js, and fail if it has drifted.

The ServiceWorker runs the same payload as the dedicated Worker. If the two ever
diverge, the capture reports a cross-context disagreement that belongs to the
harness rather than to the browser — which is the one kind of false positive this
whole number cannot afford.

    tools/detect-suite/sync-sw.py            # regenerate
    tools/detect-suite/sync-sw.py --check    # exit 1 if out of date
"""
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
HEADER = """// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//
// GENERATED from WORKER_PROBE_SRC in probe.js by tools/detect-suite/sync-sw.py.
// Do not edit: a ServiceWorker that answers differently from the dedicated
// Worker would report a disagreement that is the harness's, not the browser's.
//
// It exists as a file because a ServiceWorker script must be same-origin and
// cannot be registered from a blob: URL — which is why that context read
// `__absent: TypeError` in every capture before this.
"""


def render():
    probe = (HERE / "probe.js").read_text()
    m = re.search(r"const WORKER_PROBE_SRC = `(.*?)`;", probe, re.S)
    if not m:
        print("!! WORKER_PROBE_SRC not found in probe.js", file=sys.stderr)
        sys.exit(2)
    body = m.group(1)
    body = body.replace(
        "self.onmessage = async function () {",
        "self.onmessage = async function (ev) {\n"
        "      const reply = (d) => (ev.ports && ev.ports[0] ? ev.ports[0].postMessage(d) : self.postMessage(d));",
    ).replace("self.postMessage(", "reply(")
    return HEADER + body + "\n"


def main():
    want = render()
    out = HERE / "sw-probe.js"
    if "--check" in sys.argv:
        have = out.read_text() if out.exists() else ""
        if have != want:
            print(
                "!! sw-probe.js is out of date. Run tools/detect-suite/sync-sw.py",
                file=sys.stderr,
            )
            return 1
        print("sw-probe.js matches probe.js")
        return 0
    out.write_text(want)
    print(f"wrote {out.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
