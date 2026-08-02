#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors

"""Generate shared-rs/src/locale.rs from the Chromium tree.

Every number and string in the generated table comes out of core/src. None of
it is written by hand, and that is the point: a table of "what language do they
speak in Norway" maintained by a person is a table of what that person believes,
and the whole product rests on matching what Chrome actually does.

Three sources, all in the tree:

  1. build/config/locales.gni
     `all_chrome_locales` — the UI locales Chrome ships. Nothing outside this
     list can be passed as --lang, because there is no .pak for it.

  2. components/strings/components_locale_settings_*.xtb
     IDS_ACCEPT_LANGUAGES per locale — Chrome's own default Accept-Language for
     a browser installed in that language. `de` really is "de-DE,de,en-US,en".
     A shipped locale with no translation falls back to the value in
     components/components_locale_settings.grd, which is "en-US,en"; Chrome in
     Afrikaans genuinely sends English, and this reproduces that rather than
     inventing an af-ZA list.

  3. third_party/icu/source/data/misc/supplementalData.txt
     CLDR `territoryInfo` — per territory, which languages are spoken, which are
     official, and by what share of the population. The country-to-language step.

The locale-resolution rules (es-MX to es-419, en-AU to en-GB, and the rest) are
transcribed from CheckAndResolveLocale in ui/base/l10n/l10n_util.cc, cited line
by line below, so a Chromium bump that changes them shows up as a diff here.

Usage:
    tools/locale-table/generate.py            # writes shared-rs/src/locale.rs
    tools/locale-table/generate.py --check    # fails if the file is out of date
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
SRC = ROOT / "core/src"
OUT = ROOT / "shared-rs/src/locale.rs"


def shipped_locales() -> list[str]:
    """`platform_pak_locales` for desktop, from build/config/locales.gni.

    Not `all_chrome_locales`. locales.gni:188-193 computes, for every platform
    except Android:

        platform_pak_locales = all_chrome_locales - extended_locales

    so the 27 extended locales — sq, is, mn, hy, ka, zh-HK and the rest — ship
    no .pak on macOS or Windows. Reading the wrong list put `--lang=sq` on the
    command line for an Albanian exit, and a --lang with no .pak behind it is a
    value that cannot be true: ResourceBundle falls back to en-US and the
    browser formats in a language the profile never claimed.
    """
    text = (SRC / "build/config/locales.gni").read_text(encoding="utf-8")

    def listing(name: str) -> list[str]:
        block = re.search(rf"^{name} =\s*\[(.*?)^\]", text, re.S | re.M)
        if not block:
            sys.exit(f"could not find {name} — did locales.gni change shape?")
        # Comments first. Half the entries carry one, and several of those quote
        # a locale name Chrome does NOT ship — `"es-419",  # "es-MX" in iOS`.
        # Reading them as entries put es-MX in the shipped set and sent Mexico
        # to a .pak that does not exist.
        stripped = "\n".join(l.split("#", 1)[0] for l in block.group(1).split("\n"))
        return re.findall(r'"([^"]+)"', stripped)

    everything = listing("all_chrome_locales")
    extended = set(listing("extended_locales"))
    # Pseudolocales are debug builds only and have no business here.
    pseudo = {"ar-XB", "en-XA"}
    desktop = [l for l in everything if l not in extended and l not in pseudo]

    if not 40 <= len(desktop) <= 70:
        sys.exit(
            f"{len(desktop)} desktop locales — expected roughly 55. locales.gni "
            "has changed shape and this parse is no longer reading it correctly."
        )
    return desktop


def accept_languages() -> dict[str, str]:
    """IDS_ACCEPT_LANGUAGES per locale, plus the .grd default for the rest."""
    grd = (SRC / "components/components_locale_settings.grd").read_text(encoding="utf-8")
    m = re.search(
        r'<message name="IDS_ACCEPT_LANGUAGES"[^>]*>\s*(.*?)\s*</message>', grd, re.S
    )
    if not m:
        sys.exit("could not find the IDS_ACCEPT_LANGUAGES default in the .grd")
    default = m.group(1).strip()

    out: dict[str, str] = {}
    for path in sorted((SRC / "components/strings").glob("components_locale_settings_*.xtb")):
        locale = path.name[len("components_locale_settings_") : -len(".xtb")]
        m = re.search(
            r'<translation id="IDS_ACCEPT_LANGUAGES">(.*?)</translation>',
            path.read_text(encoding="utf-8"),
            re.S,
        )
        value = m.group(1).strip() if m else ""
        # An empty or absent translation is a real state — `ms` has an empty
        # bundle — and grit falls back to English for it.
        out[locale] = value or default
    out.setdefault("en-US", default)
    return out, default


def territory_languages() -> dict[str, list[tuple[str, bool, int]]]:
    """CLDR territoryInfo: territory -> [(language, official, population share)]."""
    lines = (SRC / "third_party/icu/source/data/misc/supplementalData.txt").read_text(
        encoding="utf-8", errors="replace"
    ).split("\n")

    try:
        start = next(i for i, l in enumerate(lines) if l.strip() == "territoryInfo{")
    except StopIteration:
        sys.exit("no territoryInfo block in supplementalData.txt")
    # The next sibling block. Without this bound the parse runs on into
    # timeData{, which has the same country codes at the same indent and no
    # languages under them — every country came back empty until it did.
    end = next(i for i, l in enumerate(lines) if i > start and re.match(r"^    \w+\{$", l))

    data: dict[str, list[tuple[str, str, int]]] = {}
    country = language = None
    status = ""
    for line in lines[start + 1 : end]:
        indent = len(line) - len(line.lstrip())
        s = line.strip()
        if indent == 8 and re.match(r"^([A-Z]{2}|\d{3})\{$", s):
            country = s[:-1]
            data[country] = []
        elif indent == 12 and re.match(r"^[a-z]{2,3}(_[A-Za-z]+)*\{$", s):
            language = s[:-1]
            status = ""
        elif m := re.match(r'officialStatus\{"(\w+)"\}', s):
            # Kept as the string, not collapsed to a bool. CLDR has three
            # values and the difference decides real rows: English in Australia
            # and Spanish in Mexico are `de_facto_official`, not `official`, so
            # a bool that only counted `official` would drop both.
            status = m.group(1)
        elif country and language:
            m = re.match(r"populationShareF:int\{(\d+)\}", s)
            if m:
                data[country].append((language, status, int(m.group(1))))
                language = None
    return data


# The statuses that make a language a country's browser language.
#
# `official_regional` is excluded on purpose: Welsh is official in Wales and
# Hawaiian in Hawaii, and a British exit that announced Welsh would be a
# stranger browser than one that announced English.
OFFICIAL = {"official", "de_facto_official"}


def resolve(locale: str, shipped: set[str]) -> str | None:
    """CheckAndResolveLocale, ui/base/l10n/l10n_util.cc:408.

    Transcribed rather than approximated. Each branch cites its line so the next
    Chromium bump can be checked against it.
    """
    if locale in shipped:
        return locale

    lang, _, region = locale.partition("-")
    if region:
        tmp = lang
        if lang == "es" and region.lower() != "es":  # :432
            tmp = "es-419"
        elif lang == "pt" and region.lower() != "br":  # :440
            tmp = "pt-PT"
        elif lang == "zh":  # :446
            tmp = "zh-TW" if region.lower() in ("hk", "mo") else "zh-CN"
        elif lang == "en":  # :454
            tmp = "en-US" if region.lower() in ("lr", "ph") else "en-GB"
        if tmp in shipped:
            return tmp

    # kAliasMap, :475. "Google updater uses no, tl, iw and en for our nb, fil,
    # he, and en-US."
    aliases = {"en": "en-US", "iw": "he", "no": "nb", "pt": "pt-BR", "tl": "fil", "zh": "zh-CN"}
    if lang in aliases and aliases[lang] in shipped:
        return aliases[lang]
    if lang in shipped:
        return lang
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="fail if the output is stale")
    args = ap.parse_args()

    if not SRC.exists():
        sys.exit(f"no Chromium tree at {SRC} — run core/build/fetch.sh first")

    shipped = shipped_locales()
    shipped_set = set(shipped)
    accepts, default_accept = accept_languages()
    territories = territory_languages()

    rows = []
    for country in sorted(territories):
        if not re.match(r"^[A-Z]{2}$", country):
            continue  # UN M.49 region codes (001, 419) are not exit countries
        # Official standing first, then how many people speak it. Without the
        # standing filter the walk reached whatever was next in the list when a
        # country's own language shipped no .pak, and produced Albania → Greek,
        # Iceland → Danish and Mongolia → Chinese. Those are not conservative
        # guesses, they are wrong answers stated confidently.
        ranked = sorted(
            (x for x in territories[country] if x[1] in OFFICIAL),
            key=lambda x: -x[2],
        )
        if not ranked:
            continue

        chosen = None
        for lang, _status, _share in ranked:
            # CLDR writes zh_Hant style subtags; only the language matters here.
            base = lang.split("_")[0]
            chosen = resolve(f"{base}-{country}", shipped_set)
            if chosen:
                break
        # Chrome's own last resort, l10n_util.cc:568 — and the honest one. A
        # country whose language Chrome ships no UI for gets an English browser,
        # because that is the browser a person there would actually have
        # installed.
        if not chosen:
            chosen = "en-US"

        langs = accepts.get(chosen, default_accept).split(",")
        rows.append((country, chosen, [l.strip() for l in langs if l.strip()]))

    body = "\n".join(
        f'    ("{c}", "{ui}", &[{", ".join(chr(34) + l + chr(34) for l in langs)}]),'
        for c, ui, langs in rows
    )

    shipped_body = "\n".join(
        f'    "{l}",' for l in sorted(shipped_set)
    )

    generated = TEMPLATE.format(
        count=len(rows),
        locales=len(shipped),
        translated=len(accepts),
        body=body,
        shipped=shipped_body,
        default_accept=default_accept,
    )

    if args.check:
        current = OUT.read_text(encoding="utf-8") if OUT.exists() else ""
        if current != generated:
            print(f"{OUT.relative_to(ROOT)} is out of date — rerun {sys.argv[0]}")
            return 1
        print(f"{OUT.relative_to(ROOT)} is up to date ({len(rows)} countries)")
        return 0

    OUT.write_text(generated, encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(rows)} countries, {len(shipped)} shipped locales")
    return 0


TEMPLATE = '''//! What a browser in a given country says about its languages.
//!
//! GENERATED by tools/locale-table/generate.py from the Chromium tree. Do not
//! edit by hand — rerun the generator after a Chromium bump, and read its diff.
//!
//! {count} countries, from CLDR territoryInfo composed with Chrome's own
//! {locales} shipped UI locales and the {translated} IDS_ACCEPT_LANGUAGES
//! values it ships with them. A country whose language Chrome does not ship a
//! UI for resolves to en-US and "{default_accept}", because that is what Chrome
//! itself does there — not because we ran out of ideas.
//!
//! Why this exists: a profile leaving through Berlin while announcing
//! `en-US,en` is one subtraction away from being flagged, the same subtraction
//! that catches a wrong timezone. The timezone already follows the exit; this
//! is the other half.
//!
//! Why it is a table and not a guess: "which language do they speak in X" is a
//! question people answer confidently and wrongly. Every row here is Chrome's
//! own answer.

/// A country's browser locale: what Chrome would be installed as, and what it
/// would send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locale {{
    /// The UI locale, suitable for `--lang`. One of Chrome's shipped locales,
    /// so a .pak for it exists.
    pub ui: &'static str,
    /// navigator.languages, in order. The Accept-Language header is generated
    /// from this by the core, so the two cannot disagree.
    pub languages: &'static [&'static str],
}}

/// ISO 3166-1 alpha-2, uppercase, sorted — the form the exit resolver returns.
static BY_COUNTRY: &[(&str, &str, &[&str])] = &[
{body}
];

/// The locale for an exit country, or `None` for a country we have no row for.
///
/// `None` rather than a silent en-US: the caller decides whether to fall back,
/// and a caller that logs the miss is how a missing row gets noticed.
pub fn for_country(code: &str) -> Option<Locale> {{
    let code = code.trim();
    if code.len() != 2 {{
        return None;
    }}
    let upper = code.to_ascii_uppercase();
    BY_COUNTRY
        .binary_search_by(|(c, _, _)| c.cmp(&upper.as_str()))
        .ok()
        .map(|i| {{
            let (_, ui, languages) = BY_COUNTRY[i];
            Locale {{ ui, languages }}
        }})
}}

/// What a profile gets when the exit could not be resolved at all.
///
/// Not a neutral choice — it is the single most common browser locale, so a
/// profile that falls back joins the largest crowd rather than a small one.
pub const FALLBACK: Locale = Locale {{
    ui: "en-US",
    languages: &["en-US", "en"],
}};

/// The UI locales Chrome ships a .pak for. Sorted.
///
/// Nothing outside this list can be passed as `--lang`: ResourceBundle would
/// find no strings and fall back to en-US, giving a profile a UI locale that
/// silently disagrees with the languages it announces.
static SHIPPED: &[&str] = &[
{shipped}
];

/// The shipped UI locale for a language tag — `CheckAndResolveLocale`,
/// ui/base/l10n/l10n_util.cc:408, transcribed.
///
/// Needed for profiles that pin their own languages: the table above answers
/// for a country, and this answers for `de-AT` or `es-MX`, which no country row
/// would produce. The two must not disagree, and
/// `resolution_agrees_with_the_generated_table` holds them together.
pub fn ui_locale_for(tag: Option<&str>) -> String {{
    let Some(tag) = tag.map(str::trim).filter(|t| !t.is_empty()) else {{
        return FALLBACK.ui.to_string();
    }};
    // Chromium canonicalises to '-'; a tag arriving as de_DE is the same tag.
    let tag = tag.replace('_', "-");

    if SHIPPED.binary_search(&tag.as_str()).is_ok() {{
        return tag;
    }}

    let (lang, region) = match tag.split_once('-') {{
        Some((l, r)) => (l.to_ascii_lowercase(), Some(r.to_ascii_lowercase())),
        None => (tag.to_ascii_lowercase(), None),
    }};

    if let Some(region) = region.as_deref() {{
        let narrowed = match lang.as_str() {{
            // :432 — es-RR other than es-ES is Latin American Spanish.
            "es" if region != "es" => "es-419".to_string(),
            // :440 — pt-RR other than pt-BR is European Portuguese.
            "pt" if region != "br" => "pt-PT".to_string(),
            // :446 — Hong Kong and Macao take Traditional, everything else
            // Simplified.
            "zh" => if region == "hk" || region == "mo" {{ "zh-TW" }} else {{ "zh-CN" }}.to_string(),
            // :454 — "Map Liberian and Filipino English to US English, and
            // everything else to British English."
            "en" => if region == "lr" || region == "ph" {{ "en-US" }} else {{ "en-GB" }}.to_string(),
            _ => lang.clone(),
        }};
        if SHIPPED.binary_search(&narrowed.as_str()).is_ok() {{
            return narrowed;
        }}
    }}

    // kAliasMap, :475. "Google updater uses no, tl, iw and en for our nb, fil,
    // he, and en-US."
    let aliased = match lang.as_str() {{
        "en" => Some("en-US"),
        "iw" => Some("he"),
        "no" => Some("nb"),
        "pt" => Some("pt-BR"),
        "tl" => Some("fil"),
        "zh" => Some("zh-CN"),
        _ => None,
    }};
    if let Some(a) = aliased {{
        if SHIPPED.binary_search(&a).is_ok() {{
            return a.to_string();
        }}
    }}
    if SHIPPED.binary_search(&lang.as_str()).is_ok() {{
        return lang;
    }}
    // :568 — "Fallback on en-US."
    FALLBACK.ui.to_string()
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn the_table_is_sorted_and_unique() {{
        // binary_search_by depends on it, and a generator bug would otherwise
        // show up as a country that silently has no row.
        for pair in BY_COUNTRY.windows(2) {{
            assert!(
                pair[0].0 < pair[1].0,
                "{{}} and {{}} are out of order or duplicated",
                pair[0].0,
                pair[1].0
            );
        }}
    }}

    #[test]
    fn every_row_has_a_language() {{
        for (country, ui, langs) in BY_COUNTRY {{
            assert!(!langs.is_empty(), "{{country}} has no languages");
            assert!(!ui.is_empty(), "{{country}} has no ui locale");
            // A list whose first entry is not a real tag would reach
            // Accept-Language generation and produce a malformed header.
            assert!(
                langs[0].len() >= 2 && langs[0].is_ascii(),
                "{{country}} starts with {{:?}}",
                langs[0]
            );
        }}
    }}

    #[test]
    fn lookup_is_case_insensitive_and_rejects_nonsense() {{
        assert_eq!(for_country("de"), for_country("DE"));
        assert!(for_country("DE").is_some());
        assert!(for_country("").is_none());
        assert!(for_country("D").is_none());
        assert!(for_country("DEU").is_none());
        assert!(for_country("ZZ").is_none());
    }}

    #[test]
    fn the_rows_agree_with_chrome() {{
        // Spot checks against values read out of the Chromium tree by hand, so
        // a generator that starts emitting plausible nonsense is caught.
        let de = for_country("DE").unwrap();
        assert_eq!(de.ui, "de");
        assert_eq!(de.languages, &["de-DE", "de", "en-US", "en"]);

        // Latin America is not Spain: CheckAndResolveLocale maps es-RR to
        // es-419, whose list is just "es-419,es" with no English at all.
        let mx = for_country("MX").unwrap();
        assert_eq!(mx.ui, "es-419");
        assert_eq!(mx.languages, &["es-419", "es"]);

        // Brazil keeps its own locale; Portugal takes pt-PT.
        assert_eq!(for_country("BR").unwrap().ui, "pt-BR");
        assert_eq!(for_country("PT").unwrap().ui, "pt-PT");

        // en-AU has no .pak, and Chrome maps it to British rather than US.
        // Australia only reaches English through CLDR's `de_facto_official`,
        // so a table built by counting `official` alone loses it.
        assert_eq!(for_country("AU").unwrap().ui, "en-GB");
        assert_eq!(for_country("US").unwrap().ui, "en-US");
    }}

    #[test]
    fn a_country_chrome_ships_no_ui_for_gets_english() {{
        // Albania's own language is official and real, and Chrome ships no
        // sq.pak on desktop — sq is in `extended_locales`. The first version of
        // this table sent it `--lang=sq` anyway, and a --lang with no .pak
        // behind it is a value that cannot be true: ResourceBundle silently
        // falls back to en-US.
        //
        // The second version fixed the .pak set and then walked past Albanian
        // to the next language CLDR lists for Albania, which is Greek. A
        // browser announcing Greek from a Tirana exit is a worse answer than an
        // English one, and stated with the same confidence. Both are pinned
        // here because both were shipped.
        for country in ["AL", "AM", "GE", "AZ", "IS", "MN"] {{
            let l = for_country(country).unwrap();
            assert_eq!(l.ui, "en-US", "{{country}} should have an English browser");
            assert_eq!(l.languages, &["en-US", "en"]);
        }}

        // Hong Kong is the opposite mistake: zh-HK ships no desktop .pak, but
        // Chrome has a rule for it — zh-HK and zh-MO map to Traditional — so
        // the answer is a Chinese browser, not an English one.
        let hk = for_country("HK").unwrap();
        assert_eq!(hk.ui, "zh-TW");
        assert_eq!(hk.languages, &["zh-TW", "zh", "en-US", "en"]);
    }}

    #[test]
    fn resolution_agrees_with_the_generated_table() {{
        // Two implementations of CheckAndResolveLocale exist — the generator's,
        // which built the table, and `ui_locale_for`, which answers at runtime.
        // Every UI locale the table chose must be a fixed point of the runtime
        // one, or a profile that pins its languages gets a different .pak from
        // one that follows its exit to the same place.
        for (country, ui, _) in BY_COUNTRY {{
            assert_eq!(
                &ui_locale_for(Some(ui)),
                ui,
                "{{country}}: the table chose {{ui}} and the resolver moved it"
            );
            assert!(
                SHIPPED.binary_search(ui).is_ok(),
                "{{country}}: {{ui}} is not a locale Chrome ships a .pak for"
            );
        }}
    }}

    #[test]
    fn a_pinned_language_resolves_the_way_chrome_would() {{
        // The cases the country table can never produce, because no country
        // maps to them — someone typed them into the profile.
        assert_eq!(ui_locale_for(Some("de-AT")), "de");
        assert_eq!(ui_locale_for(Some("es-MX")), "es-419");
        assert_eq!(ui_locale_for(Some("es-ES")), "es");
        assert_eq!(ui_locale_for(Some("pt-AO")), "pt-PT");
        assert_eq!(ui_locale_for(Some("pt")), "pt-BR");
        assert_eq!(ui_locale_for(Some("zh-MO")), "zh-TW");
        assert_eq!(ui_locale_for(Some("zh-SG")), "zh-CN");
        assert_eq!(ui_locale_for(Some("en-PH")), "en-US");
        assert_eq!(ui_locale_for(Some("en-IE")), "en-GB");
        assert_eq!(ui_locale_for(Some("no")), "nb");
        assert_eq!(ui_locale_for(Some("iw")), "he");
        assert_eq!(ui_locale_for(Some("de_DE")), "de");

        // A language Chrome ships no UI for. en-US rather than a .pak that does
        // not exist — Chromium's own last resort, l10n_util.cc:568.
        assert_eq!(ui_locale_for(Some("tlh")), "en-US");
        assert_eq!(ui_locale_for(None), "en-US");
        assert_eq!(ui_locale_for(Some("   ")), "en-US");
    }}

    #[test]
    fn shipped_is_sorted() {{
        // binary_search in ui_locale_for depends on it, and an unsorted list
        // fails by returning en-US for locales that do ship.
        for pair in SHIPPED.windows(2) {{
            assert!(pair[0] < pair[1], "{{}} and {{}} out of order", pair[0], pair[1]);
        }}
    }}
}}
'''


if __name__ == "__main__":
    raise SystemExit(main())
