#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Drive a Fury profile with Playwright.

    pip install playwright          # no `playwright install` needed — see below
    FURY_API_PORT=35000 fury-agent serve
    ./playwright_example.py <profile-id>

`playwright install` downloads Playwright's own browsers, and you do not want
any of them: the whole point is to drive the profile's browser, with its
persona, its proxy and its cookies. connect_over_cdp attaches to what is already
running and downloads nothing.
"""

import sys

from fury import Fury, running

try:
    from playwright.sync_api import sync_playwright
except ImportError:
    raise SystemExit("pip install playwright")


def main(profile_id: str) -> int:
    fury = Fury()

    with running(fury, profile_id, cdp=True) as session:
        ws = session.get("ws_endpoint")
        if not ws:
            # Not a failure of this script: a team server can forbid CDP for a
            # role, and then the browser runs and there is nothing to attach to.
            print("the profile started, but CDP was not granted for it")
            return 1

        with sync_playwright() as pw:
            browser = pw.chromium.connect_over_cdp(ws)

            # The existing context, not a new one. `new_context()` would give a
            # fresh incognito profile inside the same browser: no cookies, no
            # localStorage, none of the logins this profile exists for — and,
            # worse, it would look like a brand new machine to anything that
            # checks. Always take contexts[0].
            context = browser.contexts[0]
            page = context.pages[0] if context.pages else context.new_page()

            page.goto("https://example.com", wait_until="domcontentloaded")
            print("title:", page.title())

            # What the profile actually reports, from inside the page it is
            # driving — the same values the detect-suite measures, so this is
            # also the shortest way to see the spoofing working.
            for k, v in page.evaluate("""() => ({
                userAgent: navigator.userAgent,
                platform: navigator.platform,
                languages: navigator.languages.join(','),
                timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
                cores: navigator.hardwareConcurrency,
                memory: navigator.deviceMemory,
                screen: `${screen.width}x${screen.height}`,
                webdriver: navigator.webdriver,
            })""").items():
                print(f"  {k:12} {v}")

            # No browser.close(). On a CDP connection that closes the real
            # browser out from under the agent, which then finds a process it
            # did not stop. Leaving the `with sync_playwright()` block
            # disconnects, and stopping the profile is the agent's job — which
            # `running` asks it to do on the way out, including on an exception.

    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {sys.argv[0]} <profile-id>   (./api.sh lists them)")
    sys.exit(main(sys.argv[1]))
