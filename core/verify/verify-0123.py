#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Built-in AI answers as Chrome does, and never downloads a model.

Every Chromium build has chrome://settings/ai "On-device AI" off by default,
so LanguageModel.availability() and Summarizer.availability() answer
"unavailable" where the Google Chrome a persona claims to be answers
"downloadable" (issue 32). 0123 turns the setting on by default and refuses
the model components, so create() cannot fetch several GB of Gemini Nano per
profile through the profile's proxy.

Claims:

  * control: a profile whose Local State has the setting OFF answers
    "unavailable", so the probe can tell the two apart.
  * a fresh profile answers "downloadable" for both.
  * create() under a user gesture downloads nothing: no progress past 0 and
    no large file in the profile after WAIT seconds.
  * availability() then says "downloading", as Chrome does while the model
    comes down, and never "available".
  * with a Google Chrome path as the second argument, the answers match it.

Usage: core/verify/verify-0123.py <core binary> [google chrome binary]
"""

import json
import os
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]
CHROME = sys.argv[2] if len(sys.argv) > 2 else None
WAIT = 30
CONFIG = {"schema_version": 1}

AVAILABILITY = """(async () => {
  const out = {};
  for (const n of ['LanguageModel', 'Summarizer']) {
    try { out[n] = self[n] ? await self[n].availability() : 'absent'; }
    catch (e) { out[n] = 'threw: ' + e; }
  }
  return JSON.stringify(out);
})()"""

CREATE = """(() => {
  window.__fury = {progress: [], result: null};
  LanguageModel.create({monitor(m) {
    m.addEventListener('downloadprogress', e => window.__fury.progress.push(e.loaded));
  }}).then(() => { window.__fury.result = 'created'; },
           e => { window.__fury.result = 'error: ' + e; });
  return 'started';
})()"""


def big_files(root, limit=10 * 1024 * 1024):
    out = []
    for d, _, files in os.walk(root):
        for f in files:
            p = os.path.join(d, f)
            try:
                if os.path.getsize(p) > limit:
                    out.append((os.path.relpath(p, root), os.path.getsize(p)))
            except OSError:
                pass
    return out


def main():
    claims = Claims("0123 — built-in AI", CORE)

    off = tempfile.mkdtemp(prefix="fury-verify-ai-off-")
    with open(os.path.join(off, "Local State"), "w") as f:
        json.dump({"optimization_guide": {
            "on_device_foundational_model_user_settings": False}}, f)
    with launch(CORE, CONFIG, user_data_dir=off) as s:
        got = json.loads(s.js(AVAILABILITY))
    print(f"  setting off: {got}")
    claims.control(got["LanguageModel"] == "unavailable",
                   f"with On-device AI switched off in Local State, "
                   f"LanguageModel reads unavailable ({got})")

    with launch(CORE, CONFIG) as s:
        before = json.loads(s.js(AVAILABILITY))
        print(f"  fresh profile: {before}")
        claims.check(before == {"LanguageModel": "downloadable",
                                "Summarizer": "downloadable"},
                     f"a fresh profile reads downloadable for both, as Chrome "
                     f"does ({before})")

        r = s.ws.call("Runtime.evaluate",
                      {"expression": CREATE, "userGesture": True,
                       "returnByValue": True}, session=s.session)
        print(f"  create(): {r['result'].get('value')}, waiting {WAIT} s")
        time.sleep(WAIT)
        state = json.loads(s.js("JSON.stringify(window.__fury)"))
        big = big_files(s.dir)
        print(f"  after {WAIT} s: progress={state['progress'][-5:]} "
              f"result={state['result']} files>10MB={big}")
        claims.check(all(p == 0 for p in state["progress"]),
                     f"create() reports no download progress past 0 "
                     f"({len(state['progress'])} events, last "
                     f"{state['progress'][-1:] or 'none'})")
        claims.check(not big,
                     f"and nothing large landed in the profile ({big})")
        claims.check(state["result"] != "created",
                     f"create() did not produce a session ({state['result']})")

        # Chrome answers "downloading" for a model whose download create()
        # started, and "available" once it is there. Ours never gets there.
        after = json.loads(s.js(AVAILABILITY))
        claims.check(after["LanguageModel"] == "downloading" and "available" not in after.values(),
                     f"availability() then says the download is under way, as "
                     f"Chrome does, and never that the model is there ({after})")

    if CHROME:
        with launch(CHROME, None) as s:
            ref = json.loads(s.js(AVAILABILITY))
        claims.check(ref == before,
                     f"Google Chrome on this machine answers the same "
                     f"({ref} vs {before})")
    else:
        print("  no Google Chrome path given; reference comparison skipped")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
