#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Patch 0136: what a phone's Chrome has and a desktop's does not, and back.

Two things two captured phones (Realme RMX5131, Huawei P30) answered and a
phone persona on the desktop core did not:

  * window.chrome is {csi, loadTimes} on Android, with no chrome.app: Chrome
    on Android has no extensions system. Every desktop Chrome has chrome.app
    on every page.
  * navigator.keyboard.getLayoutMap() resolves to an empty map on Android. A
    desktop resolves to the host's layout, 48 keys of it.

Each is checked on a phone config and a desktop one, and must differ.

Usage: core/verify/verify-0136.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

READ = """
(async () => {
  const keys = window.chrome ? Object.keys(window.chrome).sort() : null;
  let layout = null;
  try { layout = (await navigator.keyboard.getLayoutMap()).size; }
  catch (e) { layout = 'error ' + e.name; }
  return JSON.stringify({keys, app: !!(window.chrome && window.chrome.app), layout});
})()
"""


def main():
    claims = Claims("0136 — a phone's chrome object and keyboard", CORE)
    with launch(CORE, None) as s:
        desk = json.loads(s.js(READ))
    with launch(CORE, {"mobile": {"enabled": True, "connectionType": "wifi", "touchSeed": 1}}) as s:
        phone = json.loads(s.js(READ))
    print(f"  desktop {desk}\n  phone   {phone}")
    claims.control(desk["app"] and isinstance(desk["layout"], int) and desk["layout"] > 0,
                   f"a desktop config has chrome.app and the host's keyboard layout "
                   f"({desk['keys']}, {desk['layout']} keys)")
    claims.check(not phone["app"] and phone["keys"] == ["csi", "loadTimes"],
                 f"a phone has window.chrome with csi and loadTimes and no app, as Chrome "
                 f"on Android ({phone['keys']})")
    claims.check(phone["layout"] == 0,
                 f"and an empty keyboard layout map ({phone['layout']})")
    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
