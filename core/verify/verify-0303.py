#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""--fury-lock-data-export, exercised through the APIs an operator would use.

An RBAC rule that says "this operator may not export the profile's passwords"
is worth exactly as much as the code that refuses. 0302 is the cautionary tale
in this same directory: the switch was pushed on every restricted launch and
nothing in the core read it, so the restriction was a hidden button for as long
as nobody looked.

0303 refuses in three places, and the choice of places is the design:

  * `PasswordManagerPorter::Export` — the function, not the button. The
    settings page reaches it over Mojo, and a replayed message or an extension
    holding `passwordsPrivate` reaches it with no button at all.
  * `PasswordsPrivateDelegateImpl::RequestPlaintextPassword` — because reading
    credentials out one at a time is the same leak as exporting them in a
    batch, only slower.
  * `bookmark_html_writer` — the single function both bookmark export callers
    reach.

So this drives the real APIs from the WebUI pages that own them, rather than
asserting that a switch string appears somewhere. The control is the same
browser without the switch, which MUST succeed: a check that passes on a build
refusing everybody is measuring nothing.

## What this script does NOT prove, and why

The bookmark refusal is not exercised. `bookmarkManagerPrivate.export()` opens
a native file-selection dialog, and `bookmark_html_writer` is only reached once
somebody has chosen a path — so in an automated run the promise resolves and no
file is written whether the switch is set or not. Measured both ways here: two
runs, two resolutions, zero files. Asserting on that promise would be a green
tick for a code path nobody ran, which is worse than an admitted gap. Verifying
it needs a human at the dialog, or a build that stubs SelectFileDialog.

Usage: core/verify/verify-0303.py <core binary>
"""

import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# Exactly the string agent/src/launcher.rs pushes. Spelled the way the Rust
# does, on purpose: the name is written independently in two languages and
# nothing in either checks the other, so a drift would fail open, silently,
# forever. That is not a hypothetical — it is what 0302 was written to fix.
SWITCH = "--fury-lock-data-export"

# getSavedPasswordList() first: without it the delegate is never constructed
# and every call fails for a reason that has nothing to do with the patch.
EXPORT_PASSWORDS = """
(async () => {
  if (!globalThis.chrome || !chrome.passwordsPrivate) return {error: 'no API'};
  try {
    const list = await chrome.passwordsPrivate.getSavedPasswordList();
    await chrome.passwordsPrivate.exportPasswords();
    return {ok: true, saved: list.length};
  } catch (e) {
    return {ok: false, message: String(e.message || e)};
  }
})()
"""

# Adding one, then reading it back: RequestPlaintextPassword is a separate
# refusal from the export, because reading credentials out one at a time is the
# same leak as exporting them in a batch, only slower. The store writes
# asynchronously, so the list has to be polled rather than read once.
READ_ONE_PASSWORD = """
(async () => {
  if (!globalThis.chrome || !chrome.passwordsPrivate) return {error: 'no API'};
  const sleep = (ms) => new Promise(r => setTimeout(r, ms));
  try {
    await chrome.passwordsPrivate.getSavedPasswordList();
    await chrome.passwordsPrivate.addPassword({
      url: 'https://example.com', username: 'u', password: 'secret-0303',
      note: '', useAccountStore: false});
    let list = [];
    for (let i = 0; i < 20 && !list.length; i++) {
      await sleep(500);
      list = await chrome.passwordsPrivate.getSavedPasswordList();
    }
    if (!list.length) return {error: 'the password store never accepted one'};
    const p = await chrome.passwordsPrivate.requestPlaintextPassword(
      list[0].id, 'VIEW');
    return {ok: true, value: p};
  } catch (e) {
    return {ok: false, message: String(e.message || e)};
  }
})()
"""


def probe(page, expression, extra_args):
    with launch(CORE, None, page=page, extra_args=extra_args) as s:
        time.sleep(4)
        return s.js(expression, timeout=60000)


def main():
    claims = Claims("0303 — the data-export lock", CORE)

    # The control first: without the switch these must WORK, or the refusals
    # below are a browser that refuses everybody.
    free_pw = probe("chrome://password-manager/passwords", EXPORT_PASSWORDS, ())
    print(f"  passwords, no switch: {free_pw}")
    free_read = probe("chrome://password-manager/passwords", READ_ONE_PASSWORD, ())
    print(f"  read one, no switch:  {free_read}")

    claims.control(
        free_pw.get("ok") is True,
        f"without the switch the password export succeeds — the refusal below "
        f"is the switch and not a browser that refuses everybody "
        f"(got {free_pw})",
    )

    locked_pw = probe("chrome://password-manager/passwords", EXPORT_PASSWORDS,
                      (SWITCH,))
    print(f"  passwords, locked:    {locked_pw}")
    claims.check(locked_pw.get("ok") is False,
                 f"with {SWITCH} the same call is refused "
                 f"(got {locked_pw})")
    claims.check(locked_pw.get("error") != "no API",
                 "and refused by the browser rather than by the API being "
                 "absent — the operator's own page still loads")

    # The second refusal, which is a separate function from the first.
    claims.control(
        free_read.get("ok") is True and free_read.get("value") == "secret-0303",
        f"without the switch a saved password can be read back one at a time "
        f"(got {free_read})",
    )
    locked_read = probe("chrome://password-manager/passwords", READ_ONE_PASSWORD,
                        (SWITCH,))
    print(f"  read one, locked:     {locked_read}")
    claims.check(locked_read.get("ok") is False,
                 f"and with the switch that read is refused too — blocking the "
                 f"batch export alone would leave the same leak running slower "
                 f"(got {locked_read})")

    print("  NOTE — the bookmark_html_writer refusal is NOT exercised: "
          "bookmarkManagerPrivate.export() waits on a native file dialog, so "
          "no automated run reaches the guarded function either way. See the "
          "docstring.")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
