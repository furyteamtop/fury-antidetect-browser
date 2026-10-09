// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//! Which font families a profile hides, and why no others.
//!
//! A persona's `fonts` is what a capture found, and a capture only asks about
//! [`PROBED`] -- the candidate list in tools/detect-suite/probe.js. Used as an
//! allowlist it also refused every family nobody asked about, and on a real
//! Apple M5 that hid Helvetica, Times, Courier, PingFang SC, Hiragino Sans and
//! Arial Unicode MS (measured 29.09.2026). pixelscan said "Masking detected"
//! and iphey "inconsistent browser fingerprint (roadmap)" for that alone.
//!
//! So the core is told what to HIDE instead (`fontsHidden`, patch 0050):
//! families that were probed and are absent from the persona, and families
//! that only exist on another OS. Everything else resolves as it does on the
//! machine, which is what an unmeasured family should do.

/// The families a capture measures. Must equal FONT_CANDIDATES in
/// tools/detect-suite/probe.js; a test reads that file and compares.
pub const PROBED: &[&str] = &[
    // Windows-only
    "Bahnschrift", "Calibri", "Cambria", "Candara", "Consolas", "Constantia",
    "Corbel", "Ebrima", "Gadugi", "Leelawadee UI", "Malgun Gothic",
    "Microsoft JhengHei", "Microsoft YaHei", "MS Gothic", "MV Boli",
    "Nirmala UI", "Segoe UI", "Segoe UI Emoji", "Segoe UI Variable",
    "Sitka", "Sylfaen", "Tahoma", "Yu Gothic",
    // macOS-only
    "Al Bayan", "American Typewriter", "Apple Chancery", "Apple Color Emoji",
    "AppleGothic", "Avenir", "Avenir Next", "Baskerville", "Big Caslon",
    "Chalkboard", "Chalkduster", "Cochin", "Copperplate", "Didot",
    "Futura", "Geneva", "Gill Sans", "Helvetica Neue", "Herculanum",
    "Hoefler Text", "Lucida Grande", "Marker Felt", "Menlo", "Monaco",
    "Optima", "Papyrus", "Phosphate", "Rockwell", "SF Pro", "Skia",
    "Snell Roundhand", "Zapfino",
    // Cross-platform / bundled with apps
    "Arial", "Arial Black", "Comic Sans MS", "Courier New", "Georgia",
    "Impact", "Times New Roman", "Trebuchet MS", "Verdana", "Webdings",
    "Wingdings", "Roboto", "Open Sans", "Inter",
];

/// Shipped with macOS and not with Windows, beyond the probed ones. Hidden
/// from a Windows persona, whatever the host is: a Windows machine that can
/// render PingFang or Helvetica is a Mac.
const MAC_ONLY: &[&str] = &[
    "Helvetica", "Times", "Courier", "PingFang SC", "PingFang TC", "PingFang HK",
    "Hiragino Sans", "Hiragino Kaku Gothic ProN", "Hiragino Mincho ProN",
    "Hiragino Sans GB", "Apple SD Gothic Neo", "Apple Symbols", "Heiti SC",
    "Heiti TC", "STHeiti", "Songti SC", "Songti TC", "Kaiti SC", "Thonburi",
    "Kohinoor Devanagari", "Kohinoor Bangla", "Kailasa", "Noteworthy",
    "Savoye LET", "Luminari", "Trattatello", "SignPainter", "Bradley Hand",
    "Charter", "Iowan Old Style", "Palatino", "Seravek", "Superclarendon",
    "SF Mono", "SF Pro Text", "SF Pro Display", "New York", "Galvji",
    "Academy Engraved LET", "Party LET", "Krungthep", "Sathu", "Silom",
];

/// Shipped with Windows and not with macOS, beyond the probed ones. Hidden
/// from a macOS persona, whatever the host is.
///
/// Microsoft Sans Serif is NOT here although the name says Windows: macOS
/// ships it in /System/Library/Fonts/Supplemental. Hidden, it was the one
/// family on the list a real Mac has, and iphey said "inconsistent browser
/// fingerprint (roadmap)" for that alone; taken off, "no signals" (measured
/// 30.09.2026 on the macOS 155 core, config branches one at a time).
const WINDOWS_ONLY: &[&str] = &[
    "Segoe UI Symbol", "Segoe UI Historic", "Segoe Print", "Segoe Script",
    "Segoe MDL2 Assets", "Segoe Fluent Icons", "Lucida Console",
    "Lucida Sans Unicode", "MS Sans Serif", "MS Serif",
    "MS PGothic", "MS UI Gothic", "Meiryo", "Meiryo UI", "SimSun", "NSimSun",
    "SimHei", "KaiTi", "FangSong", "Microsoft Himalaya", "Microsoft New Tai Lue",
    "Microsoft PhagsPa", "Microsoft Tai Le", "Microsoft Yi Baiti",
    "Mongolian Baiti", "Myanmar Text", "Javanese Text", "Ink Free",
    "HoloLens MDL2 Assets", "Marlett", "Franklin Gothic Medium", "Gabriola",
    "Palatino Linotype", "Batang", "Gulim", "Dotum", "Gungsuh",
    "Cascadia Code", "Cascadia Mono", "Yu Mincho", "Leelawadee",
];

/// What a persona of `os` with `present` fonts hides: probed and absent, plus
/// the other OS's own families. Sorted, so the config is stable.
pub fn hidden(os: &str, present: &[String]) -> Vec<String> {
    let has = |f: &str| present.iter().any(|p| p.eq_ignore_ascii_case(f));
    // A phone has neither desktop's families: what it draws Arial with is
    // its own sans-serif, so on the host the desktop fonts must not resolve.
    let other: Vec<&str> = match os {
        "Windows" => MAC_ONLY.to_vec(),
        "macOS" => WINDOWS_ONLY.to_vec(),
        "Android" => MAC_ONLY.iter().chain(WINDOWS_ONLY).copied().collect(),
        _ => Vec::new(),
    };
    let mut out: Vec<String> = PROBED
        .iter()
        .chain(other.iter())
        .filter(|f| !has(f))
        .map(|f| f.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probed_list_is_the_probes_own() {
        let js = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tools/detect-suite/probe.js"
        ))
        .expect("probe.js");
        let start = js.find("const FONT_CANDIDATES = [").expect("FONT_CANDIDATES");
        let body = &js[start..start + js[start..].find("];").unwrap()];
        let mut from_js: Vec<&str> = body
            .split('\'')
            .enumerate()
            .filter(|(i, _)| i % 2 == 1)
            .map(|(_, s)| s)
            .collect();
        let mut ours = PROBED.to_vec();
        from_js.sort();
        ours.sort();
        assert_eq!(ours, from_js, "PROBED drifted from probe.js FONT_CANDIDATES");
    }

    #[test]
    fn a_mac_keeps_what_it_really_has_and_loses_what_it_does_not() {
        let present: Vec<String> = ["Helvetica Neue", "Menlo", "Arial"].iter().map(|s| s.to_string()).collect();
        let h = hidden("macOS", &present);
        // Never measured, so never hidden: the M5 regression.
        for kept in ["Helvetica", "Times", "Courier", "PingFang SC", "Helvetica Neue", "Arial", "Microsoft Sans Serif"] {
            assert!(!h.iter().any(|x| x == kept), "{kept} hidden on a Mac");
        }
        // Probed and absent, or Windows-only.
        for gone in ["Calibri", "Segoe UI", "Zapfino", "Lucida Console", "SimSun"] {
            assert!(h.iter().any(|x| x == gone), "{gone} not hidden on a Mac");
        }
    }

    #[test]
    fn an_android_persona_hides_both_desktops() {
        let present: Vec<String> = ["Roboto"].iter().map(|s| s.to_string()).collect();
        let h = hidden("Android", &present);
        for gone in ["Helvetica", "Segoe UI", "Arial", "Menlo", "SimSun", "PingFang SC"] {
            assert!(h.iter().any(|x| x == gone), "{gone} not hidden on Android");
        }
        assert!(!h.iter().any(|x| x == "Roboto"));
    }

    #[test]
    fn a_windows_persona_hides_the_mac_on_any_host() {
        let present: Vec<String> = ["Segoe UI", "Calibri", "Arial"].iter().map(|s| s.to_string()).collect();
        let h = hidden("Windows", &present);
        for gone in ["Helvetica", "PingFang SC", "Helvetica Neue", "Menlo"] {
            assert!(h.iter().any(|x| x == gone), "{gone} not hidden on Windows");
        }
        assert!(!h.iter().any(|x| x == "Segoe UI" || x == "Calibri"));
    }
}
