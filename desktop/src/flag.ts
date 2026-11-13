// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

/** The flag of a two-letter country code, as the pair of regional indicator
 *  letters that spells it. Empty for anything that is not such a code, so a
 *  proxy that was never checked, or a check that answered a country by name,
 *  shows what it showed before.
 *
 *  Windows has no flags in its emoji font and draws the two letters instead,
 *  which is why the glyphs come from a font shipped with the app (styles.css,
 *  `.flag`). */
export function flag(code: string | null | undefined): string {
  const c = (code ?? "").trim().toUpperCase();
  if (!/^[A-Z]{2}$/.test(c)) return "";
  return String.fromCodePoint(0x1f1e6 + c.charCodeAt(0) - 65, 0x1f1e6 + c.charCodeAt(1) - 65);
}

/** "🇩🇪 DE · " for a proxy whose last check placed it, empty otherwise: the
 *  front of a proxy's line in a picker, so the country is readable without
 *  renaming every proxy after it (issue 27). A picker's line is plain text,
 *  so the flag cannot sit in a `.flag` span; the flag font is first in
 *  `--font` instead, limited by its unicode-range to the flag letters. */
export function countryPrefix(code: string | null | undefined): string {
  const f = flag(code);
  return f ? `${f} ${(code ?? "").trim().toUpperCase()} · ` : "";
}
