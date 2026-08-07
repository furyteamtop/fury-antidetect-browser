#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The locale, past `navigator.language` and into ICU.

Patch 0010 sets `navigator.languages` and the Accept-Language header — strings.
0081 is the other half: what ICU DOES with a locale, which is where a
string-only spoof comes apart. Every one of these is a different table inside
ICU, and a browser that announces German while formatting like English has told
a page both facts:

  * a NUMBER. German writes 1.234,56 — the separators swapped from English.
  * a DATE. `toLocaleDateString` gives 15.1.2026, not 1/15/2026.
  * a CURRENCY. German puts the symbol last, after a non-breaking space.
  * `Intl.DateTimeFormat().resolvedOptions().locale` — what ICU RESOLVED to,
    which is not what was asked for. A locale ICU has no data for falls back,
    and the fallback is readable.
  * `Intl.Collator` order, an ICU table again and a different one.

## The configuration this uses, and why it is not the obvious one

`locale.locale` is never set alone. `agent/src/launcher.rs` derives it from the
profile's first language through `fury_shared::locale::ui_locale_for`, so a
profile announcing `de-DE` is launched with `locale.locale = "de"` — the
SHIPPED locale, since Chrome has no de-DE .pak and ResourceBundle would fall
back to en-US. Setting `locale.locale = "de-DE"` here instead would test a
state no profile is ever in.

The last claim is the one this patch exists for, and the repository has the
evidence that it was once false: in `tools/detect-suite/baselines/v1.json`,
captured before 0081, `navigator.languages` reads `en-US,en` while
`locale.locale` reads `ru` — the host's language, formatting numbers the
Russian way behind an American navigator. `ctx-fury.json`, captured after, has
both at `en-US`.

Usage: core/verify/verify-0081.py <core binary>
"""

import json
import os
import sys
import unicodedata

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# What the launcher would produce for a German profile: the announced list, and
# the shipped UI locale derived from its head.
LANGUAGES = ["de-DE", "de", "en-US", "en"]
UI_LOCALE = "de"

READ = """
(async () => {
  const read = `({
    resolved: Intl.DateTimeFormat().resolvedOptions().locale,
    numberingSystem: Intl.NumberFormat().resolvedOptions().numberingSystem,
    language: navigator.language,
    number: (1234.56).toLocaleString(),
    date: new Date(Date.UTC(2026, 0, 15)).toLocaleDateString(),
    currency: (9.5).toLocaleString(undefined, {style: 'currency', currency: 'EUR'}),
    month: new Date(Date.UTC(2026, 0, 15)).toLocaleDateString(undefined, {month: 'long'}),
    weekday: new Date(Date.UTC(2026, 0, 15)).toLocaleDateString(undefined, {weekday: 'long'}),
    sorted: ['z', 'a\\u0308', 'a'].sort(new Intl.Collator().compare).join(''),
  })`;
  const out = {main: eval(read)};
  const w = new Worker(URL.createObjectURL(new Blob([`postMessage(JSON.stringify(${read}))`])));
  out.worker = JSON.parse(await new Promise((r) => { w.onmessage = (e) => r(e.data); }));
  w.terminate();
  return JSON.stringify(out);
})()
"""


def main():
    claims = Claims("0081 — ICU locale, not just the string", CORE)

    config = {
        "navigator": {"languages": LANGUAGES},
        "locale": {"locale": UI_LOCALE, "timezone": "Europe/Berlin"},
    }
    with launch(CORE, config) as s:
        both = json.loads(s.js(READ))
        got = both["main"]
        print(f"  measured: {got}")

        claims.check(got["resolved"] == UI_LOCALE,
                     f"ICU resolved to {UI_LOCALE!r} — the shipped locale, not a "
                     f"fallback to en-US (got {got['resolved']!r})")
        claims.check(got["numberingSystem"] == "latn",
                     f"the numbering system is latin, as it is for German "
                     f"(got {got['numberingSystem']!r})")

        # The five a string-only spoof gets wrong.
        claims.check(got["number"] == "1.234,56",
                     f"a number formats German — separators swapped from English "
                     f"(got {got['number']!r})")
        claims.check(got["date"] == "15.1.2026",
                     f"a date formats German, not 1/15/2026 (got {got['date']!r})")
        claims.check(got["currency"].startswith("9,50") and "€" in got["currency"],
                     f"currency puts the symbol last, the German way "
                     f"(got {got['currency']!r})")
        claims.check(got["month"] == "Januar" and got["weekday"] == "Donnerstag",
                     f"month and weekday come from the German tables "
                     f"({got['month']!r}, {got['weekday']!r})")
        # NFC first: the expression builds the umlaut as a + combining
        # diaeresis, and ICU sorts without composing it. Two strings that print
        # identically and compare unequal is not the finding this looks for.
        claims.check(unicodedata.normalize("NFC", got["sorted"]) == "aäz",
                     f"Intl.Collator sorts a-umlaut next to a, the German way "
                     f"(got {got['sorted']!r})")

        # The claim the patch exists for: ICU and navigator do not contradict.
        claims.check(got["resolved"] == got["language"].split("-")[0],
                     f"ICU's locale is the language navigator announces — the "
                     f"contradiction baselines/v1.json still has "
                     f"({got['resolved']!r} vs {got['language']!r})")

        w = both["worker"]
        print(f"  worker: {w}")
        claims.check(w["resolved"] == got["resolved"] and w["number"] == got["number"],
                     f"a Worker formats identically — it has its own ICU and its "
                     f"own global ({w['resolved']!r}, {w['number']!r})")

    with launch(CORE, None) as s:
        bare = json.loads(s.js(
            "JSON.stringify({loc: Intl.DateTimeFormat().resolvedOptions().locale,"
            " num: (1234.56).toLocaleString()})"))
        print(f"  unconfigured: {bare}")
        claims.control(bare["loc"] != UI_LOCALE and bare["num"] != "1.234,56",
                       f"an unconfigured build formats with the host's locale "
                       f"(got {bare})")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
