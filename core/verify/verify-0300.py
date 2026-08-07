#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""navigator.webdriver, while the browser is actually being driven.

The claim is narrow and the test has to be narrower, because the easy version
of it proves nothing: a browser nobody is automating reports `false` anyway.

So this launches with `--enable-automation`, the switch Puppeteer and Selenium
pass, WHILE attached over CDP — the state 0300 exists for. Without the patch
that combination is what flips `navigator.webdriver` to true and makes the
operator choose between automating an account and keeping it.

Three claims beyond the value itself:

  * an IFRAME agrees. `webdriver` is on `Navigator.prototype`, and a document
    in another frame has its own.
  * the getter is still NATIVE code. A page that reads
    `Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver').get
    .toString()` sees `[native code]` on a real Chrome and a JavaScript
    function on anything patched from userland — which is the tell that catches
    every extension-based hider.
  * `false`, not `undefined`. Deleting the property is the other common hack,
    and real Chrome has it and answers false.

The control is deliberately the same browser with the flag and WITHOUT the
config: that build must report true, because an unconfigured Fury stays honest
and because a check that passes both ways is not a check.

Usage: core/verify/verify-0300.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

DRIVEN = ["--enable-automation"]

READ = """
(() => {
  const desc = Object.getOwnPropertyDescriptor(Navigator.prototype, 'webdriver');
  const out = {
    value: navigator.webdriver,
    type: typeof navigator.webdriver,
    own: Object.prototype.hasOwnProperty.call(navigator, 'webdriver'),
    hasDescriptor: !!desc,
    getterSource: desc && desc.get ? desc.get.toString() : null,
  };
  const f = document.createElement('iframe');
  f.src = 'about:blank';
  document.body.appendChild(f);
  out.iframe = f.contentWindow.navigator.webdriver;
  f.remove();
  return JSON.stringify(out);
})()
"""


def main():
    claims = Claims("0300 — navigator.webdriver under automation", CORE)

    config = {"automation": {"hideTraces": True}}
    with launch(CORE, config, extra_args=DRIVEN) as s:
        got = json.loads(s.js(READ))
        print(f"  measured: {got}")

        claims.check(got["value"] is False,
                     f"webdriver is false with --enable-automation set and CDP "
                     f"attached (got {got['value']!r})")
        claims.check(got["type"] == "boolean",
                     f"and it is a boolean, not undefined — deleting the property "
                     f"is the other common hack (got {got['type']!r})")
        claims.check(got["hasDescriptor"] and not got["own"],
                     "the property lives on Navigator.prototype, where Chrome "
                     "keeps it, and not as an own property of navigator")
        claims.check(got["getterSource"] and "[native code]" in got["getterSource"],
                     f"the getter is native code — a JavaScript function here is "
                     f"the tell that catches userland hiders "
                     f"(got {got['getterSource']!r})")
        claims.check(got["iframe"] is False,
                     f"an iframe's own Navigator agrees (got {got['iframe']!r})")

    # The control, and it is the interesting one: the SAME flag, no config.
    with launch(CORE, None, extra_args=DRIVEN) as s:
        bare = s.js("navigator.webdriver")
        print(f"  unconfigured, same flag: {bare}")
        claims.control(bare is True,
                       f"an unconfigured build under --enable-automation reports "
                       f"true — the patch is what changes the answer, not the "
                       f"absence of automation (got {bare!r})")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
