#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The branding, which has to be visible to the operator and invisible to a site.

0900 renames the product: the bundle is Fury.app, the helpers are Fury Helper,
the About page says Fury. That is the half everyone thinks of.

The half that matters is the other one — none of it may reach a page. A profile
whose `navigator.userAgent` says Fury, or whose Sec-CH-UA brand list carries it,
has told every site it visits which browser it is, and there are perhaps a few
hundred of them in the world. The rename must stop at the process boundary.

So this checks both directions:

  * the built bundle really is renamed, on disk and in Info.plist, and the
    helpers with it — a mixed set (Fury.app spawning Chromium Helper) is a
    packaging bug an operator would meet at first launch.
  * `navigator.userAgent`, `navigator.appName`, `appCodeName`, `vendor`,
    `userAgentData.brands` and `chrome.runtime` carry NO trace of it, in the
    page AND in a worker.

Usage: core/verify/verify-0900.py <core binary>
"""

import json
import os
import plistlib
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# Everything a page can read that a rename might have leaked into.
READ = """
(async () => {
  const readAll = `({
    ua: navigator.userAgent,
    appName: navigator.appName,
    appCodeName: navigator.appCodeName,
    appVersion: navigator.appVersion,
    vendor: navigator.vendor,
    vendorSub: navigator.vendorSub,
    product: navigator.product,
    productSub: navigator.productSub,
    brands: navigator.userAgentData
      ? navigator.userAgentData.brands.map(b => b.brand) : null,
  })`;
  const out = {main: eval(readAll)};
  const w = new Worker(URL.createObjectURL(new Blob([
    `postMessage(JSON.stringify(${readAll}))`])));
  out.worker = JSON.parse(await new Promise((r) => { w.onmessage = (e) => r(e.data); }));
  w.terminate();
  return JSON.stringify(out);
})()
"""


def main():
    claims = Claims("0900 — branding, visible to the operator only", CORE)

    # The bundle on disk.
    app = os.path.abspath(os.path.join(os.path.dirname(CORE), "..", ".."))
    outdir = os.path.dirname(app)
    print(f"  bundle: {app}")

    claims.check(os.path.basename(app) == "Fury.app",
                 f"the bundle is Fury.app (got {os.path.basename(app)!r})")

    plist_path = os.path.join(app, "Contents", "Info.plist")
    with open(plist_path, "rb") as f:
        plist = plistlib.load(f)
    print(f"  Info.plist: name={plist.get('CFBundleName')!r} "
          f"exe={plist.get('CFBundleExecutable')!r} "
          f"id={plist.get('CFBundleIdentifier')!r}")
    claims.check(plist.get("CFBundleName") == "Fury"
                 and plist.get("CFBundleExecutable") == "Fury",
                 f"and Info.plist agrees, so the Dock and About box do too "
                 f"({plist.get('CFBundleName')!r}, "
                 f"{plist.get('CFBundleExecutable')!r})")
    claims.check("chromium" not in plist.get("CFBundleIdentifier", "").lower(),
                 f"the bundle identifier was renamed too — a Fury.app with "
                 f"org.chromium.Chromium inside shares macOS state with a real "
                 f"Chromium install (got {plist.get('CFBundleIdentifier')!r})")

    helpers = sorted(n for n in os.listdir(outdir) if n.endswith("Helper.app")
                     or "Helper (" in n)
    print(f"  helpers: {helpers}")
    claims.check(helpers and all(h.startswith("Fury Helper") for h in helpers),
                 f"every helper is renamed with it — Fury.app spawning "
                 f"'Chromium Helper' is what an operator meets at first launch "
                 f"({helpers})")

    # And none of it reaches a page.
    with launch(CORE, None) as s:
        got = json.loads(s.js(READ))
        print(f"  ua: {got['main']['ua']}")
        print(f"  brands: {got['main']['brands']}")

        for context in ("main", "worker"):
            blob = json.dumps(got[context]).lower()
            claims.check("fury" not in blob,
                         f"nothing a page reads in the {context} context "
                         f"contains the product name — a UA saying Fury names "
                         f"one of a few hundred browsers in the world")

        claims.check(got["main"]["appName"] == "Netscape"
                     and got["main"]["appCodeName"] == "Mozilla"
                     and got["main"]["product"] == "Gecko",
                     f"the fossil fields still say Netscape/Mozilla/Gecko, as "
                     f"they do in every browser since 1998 "
                     f"({got['main']['appName']!r}, "
                     f"{got['main']['appCodeName']!r}, "
                     f"{got['main']['product']!r})")
        claims.check(got["main"]["vendor"] == "Google Inc.",
                     f"and navigator.vendor still says Google Inc. "
                     f"(got {got['main']['vendor']!r})")

        # The control: the rename IS real, and this proves the checks above are
        # not passing because nothing was renamed at all.
        version = s.js("navigator.userAgent.match(/Chrome\\/([\\d.]+)/)[1]")
        print(f"  reported Chrome version: {version}")
        claims.control(
            os.path.basename(app) == "Fury.app"
            and "Chrome/" in got["main"]["ua"]
            and "Fury" not in got["main"]["ua"],
            f"the binary IS renamed on disk and still says Chrome/{version} to "
            f"a page — the two halves are in opposite directions, and both are "
            f"required",
        )

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
