#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Two APIs answering about one permission, and having to agree.

`Notification.permission` and `navigator.permissions.query({name:
"notifications"})` report the same underlying state through different code and
in different words — Permissions says "prompt", Notification says "default".
A browser where they disagree is the classic headless tell, and 0090's own
comment records that this patch was written after measurement produced exactly
that: query said "denied" while Notification.permission said "default",
because only one of the two had been overridden.

So the script asserts the pair, in all three states, rather than either value
alone. Each is checked in a Worker too, where `Notification.permission` does
not exist but `navigator.permissions` does — and the two contexts must not
disagree either.

The last claim is the one the patch's guard exists for: the override applies
only while the real state is still ASK. Once someone has actually clicked Allow
or Block, that answer is the truth, and overriding it would be the
contradiction rather than the fix. This is checked by GRANTING the permission
over CDP and confirming the config no longer wins.

Usage: core/verify/verify-0090.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# What the two APIs call the same state.
PAIRS = {"granted": "granted", "denied": "denied", "prompt": "default"}

READ = """
(async () => {
  const q = await navigator.permissions.query({name: 'notifications'});
  const out = {query: q.state, notification: Notification.permission};

  const src = `
    (async () => {
      const q = await navigator.permissions.query({name: 'notifications'});
      postMessage(JSON.stringify({
        query: q.state,
        hasNotification: typeof Notification !== 'undefined',
      }));
    })()`;
  const w = new Worker(URL.createObjectURL(new Blob([src])));
  out.worker = JSON.parse(await new Promise((r) => { w.onmessage = (e) => r(e.data); }));
  w.terminate();
  return JSON.stringify(out);
})()
"""


def main():
    claims = Claims("0090 — notifications, through two APIs at once", CORE)

    for want, notification_word in PAIRS.items():
        with launch(CORE, {"permissions": {"notifications": want}}) as s:
            got = json.loads(s.js(READ))
            print(f"  {want}: {got}")

            claims.check(got["query"] == want,
                         f"permissions.query reports {want!r} "
                         f"(got {got['query']!r})")
            claims.check(got["notification"] == notification_word,
                         f"and Notification.permission reports "
                         f"{notification_word!r}, the same state in the other "
                         f"API's words (got {got['notification']!r})")
            claims.check(got["worker"]["query"] == want,
                         f"a Worker's permissions.query agrees "
                         f"(got {got['worker']['query']!r})")

    # The guard: a real decision beats the config.
    with launch(CORE, {"permissions": {"notifications": "denied"}}) as s:
        s.ws.call("Browser.grantPermissions",
                  {"origin": "https://example.com", "permissions": ["notifications"]})
        after = json.loads(s.js(READ))
        print(f"  after a real grant: {after}")
        claims.check(after["query"] == "granted" and after["notification"] == "granted",
                     f"once the permission is actually granted, the config's "
                     f"'denied' no longer wins — the override applies only while "
                     f"nobody has answered (got {after})")

    with launch(CORE, None) as s:
        bare = json.loads(s.js(READ))
        print(f"  unconfigured: {bare}")
        claims.control(bare["query"] == "prompt" and bare["notification"] == "default",
                       f"an unconfigured build reports the untouched state, so "
                       f"the answers above came from the config (got "
                       f"{bare['query']!r}/{bare['notification']!r})")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
