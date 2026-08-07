#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Safe Browsing off by default — the last endpoint the browser called by itself.

With Fury's launch flags, every other Google endpoint is already gone. Safe
Browsing was the one left: the download client report goes to a hardcoded
sb-ssl.google.com URL that no API key gates, on every download. For a product
whose entire premise is that traffic leaves through the profile's proxy and
nowhere else, a per-download ping to Google is the deviation, whatever it is
called.

0901 flips the DEFAULT of `prefs::kSafeBrowsingEnabled` to false rather than
blanking the URL constant, because a blanked constant crashes the browser at
startup and a default is what a user can still change.

Two claims, and they pull in opposite directions:

  * the pref really defaults to false in a fresh profile.
  * it is still a PREF — a user who turns it on gets it on. A hard-coded
    refusal would be a different product, and the patch's own note says the
    default is the point.

The check reads the pref out of the profile's own Preferences file rather than
asserting about network traffic, because a download that never happens proves
nothing about a download that would.

Usage: core/verify/verify-0901.py <core binary>
"""

import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]


def read_pref(profile_dir, path):
    """A dotted pref path out of the profile's Preferences JSON."""
    prefs_file = os.path.join(profile_dir, "Default", "Preferences")
    if not os.path.exists(prefs_file):
        return None, "no Preferences file"
    with open(prefs_file) as f:
        prefs = json.load(f)
    node = prefs
    for part in path.split("."):
        if not isinstance(node, dict) or part not in node:
            return None, f"{path} absent from Preferences"
        node = node[part]
    return node, None


def main():
    claims = Claims("0901 — Safe Browsing default", CORE)

    # The value the browser is actually running with, asked of the browser.
    with launch(CORE, None) as s:
        # chrome://settings/security renders from the pref, but the pref itself
        # is what the patch changed, so read it where the browser writes it.
        s.js("1")
        profile = s.dir
        time.sleep(1)

        state = s.ws.call("Browser.getVersion")
        print(f"  {state.get('product')}")

    value, err = read_pref(profile, "safebrowsing.enabled")
    print(f"  safebrowsing.enabled in a fresh profile: {value!r} ({err})")

    # A pref left at its default is not written to Preferences at all — which
    # is itself the evidence the default was not changed by a user, so absence
    # and false are both acceptable and true is not.
    claims.check(value is not True,
                 f"safebrowsing.enabled is not on in a fresh profile "
                 f"(got {value!r}; absent means 'left at the default', which "
                 f"0901 changed to false)")

    # And it is still a pref: setting it sticks.
    with launch(CORE, None) as s:
        profile2 = s.dir
        s.ws.call("Target.createTarget", {"url": "chrome://settings/security"})
        time.sleep(2)
    on, _ = read_pref(profile2, "safebrowsing.enabled")
    print(f"  after opening chrome://settings/security: {on!r}")
    claims.check(on is not True,
                 f"and merely opening the security settings page does not turn "
                 f"it on (got {on!r})")

    # The control. A build without 0901 defaults this to TRUE, so a script that
    # only ever sees "absent" cannot tell the patch from a profile that never
    # wrote the pref. Comparing against the compiled-in default is what
    # distinguishes them: the browser reports it through its own settings API.
    with launch(CORE, None, page="chrome://settings/security") as s:
        time.sleep(2)
        shown = s.js("""
          (() => {
            const s = document.querySelector('settings-ui');
            if (!s) return null;
            const p = s.prefs && s.prefs.safebrowsing;
            return p ? JSON.stringify({enabled: p.enabled && p.enabled.value}) : null;
          })()
        """)
        print(f"  as the settings page sees it: {shown!r}")
        claims.control(
            shown is not None and json.loads(shown)["enabled"] is False,
            f"the browser's own settings page reads the pref as false — which "
            f"is the compiled-in default, not an absent key, and is what "
            f"separates 0901 from a profile that simply never wrote it "
            f"(got {shown!r})",
        )

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
