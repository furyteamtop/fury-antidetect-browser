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
