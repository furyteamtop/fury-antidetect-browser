#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The "Google API keys are missing" bar is gone, and only that bar.

Chromium shows it in the first window of every launch of a build without
keys, which this build is on purpose. It said "Chromium" to an operator, in
the profile's UI language, and took 56 px off the top of the page that a
real Chrome window does not take. 0902 removes the one call that shows it.

Writing this found something the bar was doing to the PAGE. Measured on the
0.1.4 core, first window, 900x700: `window.innerHeight` said 613 while
`document.documentElement.clientHeight` and `visualViewport.height` said
557. The bar is one of the "migrated" infobars (BrowserInfoBarManager), and
those are laid over the web contents rather than resizing it -- the widget
stays 613 tall, the viewport inside it is inset by 56. The old-style
bad-flags prompt resizes the widget instead: with --no-sandbox all three
numbers agreed at 557. Real Chrome, with no scrollbar, has innerHeight equal
to clientHeight. So every first window of every profile carried a 56-pixel
disagreement that one comparison finds, until it was dismissed. That is not
cosmetic, and it is the second claim below.

The bar is browser UI, not page content, so no page API names it. What a
page CAN measure is the chrome above and below it: `outerHeight` minus the
viewport it actually got. 0020 replaces `outerHeight` with a configured value
when `screen.chromeHeightDelta` is set; launched with no config at all it
answers the real number, and the real number is what this reads.

Four claims:

  * launched plainly, the chrome is the same height as with `--test-type`,
    which is Chromium's own "show no startup infobars" switch. A build with
    the bar answers 56 px more without the switch than with it.
  * `innerHeight` equals the viewport the page laid out in. A build with the
    bar answers 613 against 557.
  * the CONTROL: launched with a flag Chromium warns about, the chrome IS
    taller by an infobar. That proves the measurement sees a bar when one is
    there, and that 0902 removed one bar rather than the mechanism -- the
    bad-flags prompt is shown by real Chrome under the same conditions.
  * a second window in the same profile measures the same as the first: the
    bar was a first-window-only thing, and a script that only looked at the
    first window could be fooled by a build that shows it on the second.

All of it measured in the tab the browser OPENED WITH, not in one created
over CDP afterwards: startup infobars attach to the startup tab's own
WebContents and to no other, and the harness's tab said 87 px on a build
that shows the bar.

Usage: core/verify/verify-0902.py <core binary>
"""

import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# A page that exists and is not about:blank -- see harness.PAGE for why --
# and whose layout does not change the window's chrome.
PAGE = "https://example.com"

# A cookie key on descriptor 4, the way the agent hands one over, so the
# core never asks the keychain. Without it every launch here raised the
# "Fury wants to use Chromium Safe Storage" prompt on the operator's screen,
# and "Always allow" could not stick: a freshly built core is ad-hoc signed
# and is a new application to the keychain every time. Any 32 bytes will do;
# nothing in this script reads a cookie.
KEY = bytes(range(32))

# outerHeight minus the viewport the page actually got, in a page with no
# scrollbar. innerHeight would be the obvious second term and it is the wrong
# one here: with the bar present it disagrees with the viewport by the bar's
# height -- see the module comment.
CHROME_HEIGHT = "window.outerHeight - document.documentElement.clientHeight"
INNER_VS_VIEWPORT = "window.innerHeight - document.documentElement.clientHeight"


def measure_startup_tab(s, expression=CHROME_HEIGHT):
    """`expression`, evaluated in the tab the browser OPENED WITH."""
    for _ in range(50):
        targets = s.ws.call("Target.getTargets")["targetInfos"]
        startup = [t for t in targets if t["type"] == "page" and t["url"].startswith(PAGE)]
        if startup:
            break
        time.sleep(0.2)
    else:
        raise RuntimeError("the startup tab never appeared")
    sid = s.ws.call("Target.attachToTarget",
                    {"targetId": startup[0]["targetId"], "flatten": True})["sessionId"]
    # Infobars animate in after the first paint; give them a moment so an
    # absent bar is absent and not merely late.
    time.sleep(2)
    r = s.ws.call("Runtime.evaluate",
                  {"expression": expression, "returnByValue": True}, session=sid)
    return int(r["result"]["value"])


def chrome_height(extra_args=()):
    # PAGE on the command line makes it the startup tab; the harness's own
    # about:blank tab is created beside it and ignored.
    with launch(CORE, None, page="about:blank", extra_args=(*extra_args, PAGE), os_crypt_key=KEY) as s:
        return measure_startup_tab(s)


def main():
    claims = Claims("0902 — no API-keys infobar", CORE)

    plain = chrome_height()
    quiet = chrome_height(("--test-type",))
    print(f"  chrome height, plain launch:      {plain} px")
    print(f"  chrome height, with --test-type:  {quiet} px")
    claims.check(
        plain == quiet,
        f"a plain launch has no more chrome than one with --test-type, so no "
        f"startup infobar is shown ({plain} vs {quiet} px)",
    )

    with launch(CORE, None, page="about:blank", extra_args=(PAGE,), os_crypt_key=KEY) as s:
        gap = measure_startup_tab(s, INNER_VS_VIEWPORT)
    print(f"  innerHeight - clientHeight, first window: {gap} px")
    claims.check(
        gap == 0,
        f"innerHeight agrees with the viewport the page laid out in "
        f"(off by {gap} px; the bar made it 56)",
    )

    # The bad-flags prompt. `--no-sandbox` is on Chromium's list of flags it
    # warns about, on every platform, and the warning is an infobar in the
    # same slot the API-keys bar occupied.
    warned = chrome_height(("--no-sandbox",))
    print(f"  chrome height, with --no-sandbox: {warned} px")
    claims.control(
        warned > plain,
        f"the bad-flags prompt still raises the chrome by an infobar "
        f"({warned} vs {plain} px) — the measurement sees a bar when there is "
        f"one, and 0902 removed one bar, not the mechanism",
    )

    # First window and second window in one profile.
    with launch(CORE, None, page="about:blank", extra_args=(PAGE,), os_crypt_key=KEY) as s:
        first = measure_startup_tab(s)
        second_id = s.ws.call("Target.createTarget", {"url": PAGE, "newWindow": True})["targetId"]
        time.sleep(2)
        sid = s.ws.call("Target.attachToTarget", {"targetId": second_id, "flatten": True})["sessionId"]
        r = s.ws.call("Runtime.evaluate", {"expression": CHROME_HEIGHT, "returnByValue": True}, session=sid)
        second = int(r["result"]["value"])
    print(f"  first window {first} px, second window {second} px")
    claims.check(
        first == second == plain,
        f"the second window in the profile measures the same as the first "
        f"({first} vs {second} px): the bar is not merely moved to a later window",
    )

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
