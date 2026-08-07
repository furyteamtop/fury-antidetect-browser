#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""A cookie jar that travels with the profile rather than with the machine.

Chromium encrypts cookie values under a key that belongs to the MACHINE — a
random keychain password on macOS, DPAPI on Windows. That is the right answer
for a browser installed on one computer and the wrong one for a product whose
whole purpose is packing a profile into a bundle and handing it to a colleague.
On the other machine the key does not exist, and SQLitePersistentCookieStore
does not gracefully skip the rows it cannot read.

0110 supplies a key that travels in the bundle. This tests it the only way that
means anything: write a cookie, shut the browser down, and start it again.

  * SAME key, same profile → the cookie comes back. That is the feature.
  * DIFFERENT key, same profile → the cookie does NOT come back, and the
    browser still starts. This is the honest half. A wrong key must degrade to
    an empty jar and a login prompt, not to a browser that will not run; the
    patch's own note says a profile that cannot start is unrecoverable while
    one that lost its cookies is a login away.
  * the value is really ENCRYPTED on disk — the plaintext must not be findable
    in the Cookies file, or none of the above was about encryption at all.

Usage: core/verify/verify-0110.py <core binary>
"""

import json
import os
import shutil
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

KEY_A = bytes(range(32))
KEY_B = bytes(range(100, 132))

# Distinctive enough to grep the database for.
COOKIE_VALUE = "fury-verify-0110-plaintext-marker"

SET = """
(async () => {
  document.cookie = 'furyverify=%s; path=/; max-age=86400';
  return document.cookie;
})()
""" % COOKIE_VALUE

GET = "document.cookie"


def cookies_file(profile):
    return os.path.join(profile, "Default", "Cookies")


def main():
    claims = Claims("0110 — a cookie key that travels with the profile", CORE)

    profile = tempfile.mkdtemp(prefix="fury-verify-0110-")
    try:
        with launch(CORE, None, os_crypt_key=KEY_A, user_data_dir=profile) as s:
            wrote = s.js(SET)
            print(f"  wrote: {wrote}")
            claims.check(COOKIE_VALUE in wrote,
                         "the cookie was set in the first session")
            # Let the store flush before the process goes away.
            time.sleep(2)

        db = cookies_file(profile)
        claims.check(os.path.exists(db),
                     f"and a Cookies database exists on disk (got {db})")
        raw = open(db, "rb").read()
        print(f"  Cookies file: {len(raw)} bytes")
        claims.check(COOKIE_VALUE.encode() not in raw,
                     "whose bytes do NOT contain the plaintext — otherwise "
                     "nothing below is about encryption at all")

        # Same key, same profile.
        with launch(CORE, None, os_crypt_key=KEY_A, user_data_dir=profile) as s:
            again = s.js(GET)
            print(f"  same key:      {again!r}")
            claims.check(COOKIE_VALUE in again,
                         "the same key reads the cookie back — the profile "
                         "travels with its jar")

        # Different key, same profile.
        with launch(CORE, None, os_crypt_key=KEY_B, user_data_dir=profile) as s:
            other = s.js(GET)
            print(f"  different key: {other!r}")
            claims.check(COOKIE_VALUE not in other,
                         f"a different key does not read it (got {other!r})")
            claims.check(s.js("1 + 1") == 2,
                         "and the browser is still running — a wrong key has to "
                         "degrade to an empty jar and a login prompt, because a "
                         "profile that will not start is unrecoverable")

        # The control: no key at all. This is the machine-key path, which is
        # what an unpatched Chromium does and what a profile predating team
        # sync still does.
        with launch(CORE, None, user_data_dir=profile) as s:
            unkeyed = s.js(GET)
            print(f"  no key at all: {unkeyed!r}")
            claims.control(
                COOKIE_VALUE not in unkeyed and s.js("1 + 1") == 2,
                f"with no key the jar falls back to this machine's own OS-crypt "
                f"key, cannot read what KEY_A wrote, and still starts — so the "
                f"read above came from the key and not from the file merely "
                f"being present (got {unkeyed!r})",
            )
    finally:
        shutil.rmtree(profile, ignore_errors=True)

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
