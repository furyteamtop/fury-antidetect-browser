// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! fury-detect — compare fingerprint dumps produced by probe.js.
//!
//! Phase 0 workflow (docs/09-roadmap.md):
//!
//!   1. Open probe.html in real Chrome          -> baselines/chrome-<ver>-<os>.json
//!   2. Open probe.html in the build under test -> candidate.json
//!   3. fury-detect diff --mode spoof baselines/chrome-151-macos.json candidate.json
//!
//! Exit code is 0 only when nothing that matters differs, so this drops straight
//! into CI as the release gate.

mod classify;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use classify::{classify, Kind};
use serde_json::Value;

mod persona;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("diff") => cmd_diff(&args[1..]),
        Some("gate") => cmd_gate(&args[1..]),
        Some("flatten") => cmd_flatten(&args[1..]),
        Some("redact") => cmd_redact(&args[1..]),
        Some("persona") => persona::cmd_persona(&args[1..]),
        _ => {
            eprintln!(
                "fury-detect {}\n\
                 \n\
                 Compare fingerprint dumps captured with tools/detect-suite/probe.html\n\
                 \n\
                 USAGE:\n  \
                   fury-detect diff [--mode spoof|identity] <baseline.json> <candidate.json>\n      \
                     identity  same browser twice: nothing may differ\n      \
                     spoof     real Chrome vs Fury: values may differ, behaviour may not\n\
                 \n  \
                   fury-detect persona <capture.json> [--id name] [--weight 0.01]\n      \
                     Turn a probe capture into a persona for the catalogue.\n\
                 \n  \
                   fury-detect redact --check [dir]\n      \
                     Fail if any baseline still carries a routable address.\n\
                 \n  \
                   fury-detect gate <dump.json>\n      \
                     Release-gate checks on a single dump. Non-zero exit on failure.\n\
                 \n  \
                   fury-detect flatten <dump.json>\n      \
                     Print the dump as sorted dotted paths.\n\
                 \n  \
                   fury-detect redact <in.json> <out.json>\n      \
                     Strip network-identifying fields so a reference baseline\n      \
                     can be committed. Required before publishing one.\n",
                env!("CARGO_PKG_VERSION")
            );
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------------------
// flatten
// ---------------------------------------------------------------------------

/// Collapse a nested dump into `dotted.path -> scalar`.
///
/// Arrays become a single joined scalar rather than indexed paths: the order of
/// a font list or an extension list is itself the value, and `fonts.[17]` diffs
/// are unreadable when one element is inserted.
fn flatten(value: &Value, prefix: &str, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(v, &path, out);
            }
        }
        Value::Array(items) => {
            let joined = items
                .iter()
                .map(|i| match i {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect::<Vec<_>>()
                .join(",");
            out.insert(prefix.to_string(), joined);
        }
        Value::Null => {
            out.insert(prefix.to_string(), "null".into());
        }
        Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        other => {
            out.insert(prefix.to_string(), other.to_string());
        }
    }
}

fn load(path: &str) -> Result<BTreeMap<String, String>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {path}"))?;
    let json: Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {path} — is it a probe.html dump?"))?;
    let mut out = BTreeMap::new();
    flatten(&json, "", &mut out);
    Ok(out)
}

fn cmd_flatten(args: &[String]) -> Result<()> {
    let path = args.first().context("usage: fury-detect flatten <dump.json>")?;
    for (k, v) in load(path)? {
        println!("{k} = {v}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// redact
// ---------------------------------------------------------------------------

/// Strip fields that identify the machine's network rather than its hardware.
///
/// A reference baseline has to be committable — that is its whole purpose, CI
/// compares against it. But a raw dump contains the capturing machine's public
/// IP, harvested from WebRTC ICE candidates. Publishing that to a public repo
/// discloses the operator's real address, which for an anti-detect project is
/// precisely the thing it exists to protect.
///
/// SUBSTITUTES rather than deletes, which the first version did not.
///
/// Deleting `webrtc.addresses` takes the address out and takes the finding with
/// it. That field is the evidence that an unpatched build hands a routable
/// address to any page that opens a peer connection — the measurement patch 0070
/// exists because of — and a redacted baseline with the field missing cannot
/// tell a reader whether the browser leaked or simply gathered nothing. Those
/// are opposite results.
///
/// So a routable address becomes 203.0.113.7 (RFC 5737 TEST-NET-3) or
/// 2001:db8::7 (RFC 3849). Both are reserved for documentation and belong to
/// nobody. The candidate count, the candidate types, the mDNS name and the
/// shape all survive; only the address does not.
///
/// `webrtc.raw` is still dropped: it is a hash of the candidate lines as they
/// were, which is a hash of the address, and a 32-bit FNV over the routable
/// IPv4 space is not a one-way function in any useful sense.
///
/// Hardware fields (GPU model, screen size, fonts, timezone) are deliberately
/// left alone: they ARE the baseline. Anyone publishing one should understand
/// they are describing their own machine.
fn is_private_v4(a: &str) -> bool {
    let p: Vec<u32> = a.split('.').filter_map(|x| x.parse().ok()).collect();
    if p.len() != 4 || p.iter().any(|&x| x > 255) {
        return true; // a dotted quad that is not an address: a version string
    }
    p[0] == 0
        || p[0] == 10
        || p[0] == 127
        || (p[0] == 192 && p[1] == 168)
        || (p[0] == 172 && (16..=31).contains(&p[1]))
        || (p[0] == 169 && p[1] == 254)
        || (p[0] == 100 && (64..=127).contains(&p[1]))
        || p[0] >= 224
        // The whole documentation block, not just the address we substitute.
        // RFC 5737 reserves 192.0.2.0/24, 198.51.100.0/24 and 203.0.113.0/24,
        // and treating only our own placeholder as safe means redacting an
        // already-redacted file reports changes it did not need to make.
        || (p[0] == 203 && p[1] == 0 && p[2] == 113)
        || (p[0] == 192 && p[1] == 0 && p[2] == 2)
        || (p[0] == 198 && p[1] == 51 && p[2] == 100)
}

fn is_private_v6(a: &str) -> bool {
    let low = a.to_ascii_lowercase();
    low.starts_with("fe80")
        || low.starts_with("fc")
        || low.starts_with("fd")
        || low.starts_with("::")
        || low.starts_with("2001:db8")
        || low.matches(':').count() < 2
}

/// Rewrites routable addresses in one string. Returns how many it replaced.
fn scrub(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut n = 0;
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    // Hand-rolled rather than regex: this crate has no regex dependency and
    // adding one to redact a field would be the wrong trade.
    while i < bytes.len() {
        let is_tok = |c: char| c.is_ascii_hexdigit() || c == '.' || c == ':';
        if is_tok(bytes[i]) && (i == 0 || !is_tok(bytes[i - 1])) {
            let mut j = i;
            while j < bytes.len() && is_tok(bytes[j]) {
                j += 1;
            }
            let tok: String = bytes[i..j].iter().collect();
            let dots = tok.matches('.').count();
            let colons = tok.matches(':').count();
            if colons == 0 && dots == 3 && !is_private_v4(&tok) {
                out.push_str("203.0.113.7");
                n += 1;
                i = j;
                continue;
            }
            // A real IPv6 address either uses the :: compression or has all
            // eight groups. A clock does not. "01:00:00" is three groups of
            // hex-looking digits with no compression, and the first version of
            // this rewrote it — turning a timestamp inside a locale capture
            // into "2001:db8::7" and corrupting the measurement it was
            // supposed to be protecting.
            let looks_v6 = tok.contains("::") || colons >= 7;
            if looks_v6 && colons >= 2 && dots == 0 && !is_private_v6(&tok) {
                out.push_str("2001:db8::7");
                n += 1;
                i = j;
                continue;
            }
            out.push_str(&tok);
            i = j;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    (out, n)
}

/// Scrubs only the WebRTC subtree.
///
/// The first version walked every string in the document, and it corrupted the
/// capture. "Chrome/150.0.0.0" in the user agent is four dotted decimals none of
/// which exceeds 255, so it was rewritten to "Chrome/203.0.113.7" — in
/// navigator.userAgent, navigator.appVersion, the client-hints brand list and
/// all five per-context copies. The single most important field in a
/// fingerprint capture, silently replaced, by the tool whose job was to make
/// the capture safe to publish.
///
/// A version string and an IP address are the same shape, and no amount of
/// cleverness about octet ranges separates them — 8.0.0.0 is a real brand
/// version and a valid address. So the answer is not a better pattern, it is a
/// narrower scope: ICE candidates are the only place a capture holds a real
/// address, and probe.js puts them in exactly one place.
fn scrub_webrtc(json: &mut Value) -> usize {
    match json.get_mut("webrtc") {
        Some(w) => scrub_value(w),
        None => 0,
    }
}

fn scrub_value(v: &mut Value) -> usize {
    match v {
        Value::String(s) => {
            let (new, n) = scrub(s);
            *s = new;
            n
        }
        Value::Array(a) => a.iter_mut().map(scrub_value).sum(),
        Value::Object(o) => o.values_mut().map(scrub_value).sum(),
        _ => 0,
    }
}

fn cmd_redact(args: &[String]) -> Result<()> {
    // `--check` sweeps the whole baselines directory and fails if any dump still
    // carries a routable address. That mode is the durable half of this command:
    // a capture is produced by running a browser on somebody's real machine, so
    // the problem recurs on every measurement anyone ever takes, and a rule that
    // lives only in a .gitignore comment is a rule until somebody uses `add -f`.
    if args.first().map(String::as_str) == Some("--check") {
        let dir = args
            .get(1)
            .cloned()
            .unwrap_or_else(|| "tools/detect-suite/baselines".to_string());
        let mut leaked = Vec::new();
        for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {dir}"))? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path)?;
            let mut json: Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let n = scrub_webrtc(&mut json);
            if n > 0 {
                leaked.push((path, n));
            }
        }
        for (p, n) in &leaked {
            eprintln!("  {}: {n} routable address(es)", p.display());
        }
        if !leaked.is_empty() {
            anyhow::bail!(
                "{} baseline(s) carry a routable address. Redact before committing.",
                leaked.len()
            );
        }
        println!("no baseline carries a routable address");
        return Ok(());
    }

    let (input, output) = match args {
        [i, o] => (i, o),
        _ => anyhow::bail!(
            "usage: fury-detect redact <in.json> <out.json>\n\
             \x20      fury-detect redact --check [dir]"
        ),
    };

    let text = std::fs::read_to_string(input).with_context(|| format!("reading {input}"))?;
    let mut json: Value = serde_json::from_str(&text)?;

    let replaced = scrub_webrtc(&mut json);

    let mut removed = Vec::new();
    if let Some(webrtc) = json.get_mut("webrtc").and_then(|v| v.as_object_mut()) {
        if webrtc.remove("raw").is_some() {
            removed.push("webrtc.raw".to_string());
        }
        webrtc.insert("__redacted".into(), Value::Bool(true));
    }

    json.as_object_mut()
        .context("dump is not a JSON object")?
        .insert("__redacted".into(), Value::Bool(true));

    std::fs::write(output, serde_json::to_string_pretty(&json)? + "\n")?;

    println!("redacted {} -> {}", input, output);
    println!("  {replaced} routable address(es) replaced with documentation addresses");
    for r in &removed {
        println!("  removed {r}");
    }
    println!("\nStill present, because a baseline is meaningless without them:");
    println!("  GPU model, screen size, font list, timezone, CPU and memory counts,");
    println!("  and the ICE candidate types — which is the WebRTC finding itself.");
    println!("Publishing a baseline describes the machine that captured it.");
    Ok(())
}

#[cfg(test)]
mod redact_tests {
    use super::*;

    #[test]
    fn a_version_string_is_not_an_address() {
        // The bug this exists to prevent, in the exact shape it shipped in:
        // Chrome/150.0.0.0 is four dotted decimals, none above 255, none in any
        // private range — and it is the single most important field in a
        // capture. The scrubber rewrote it in navigator.userAgent,
        // navigator.appVersion, the client-hints brand list and all five
        // per-context copies, across thirty files, and the tool that did it was
        // the one whose job was to make the capture safe to publish.
        //
        // No pattern separates a version from an address: 8.0.0.0 is both.
        // These assert the scope, which is what actually fixes it.
        let mut doc: Value = serde_json::json!({
            "navigator": {
                "userAgent": "Mozilla/5.0 (Windows NT 10.0) Chrome/150.0.0.0 Safari/537.36",
            },
            "clientHints": { "fullVersionList": "Not;A=Brand/8.0.0.0,Chromium/150.0.7871.187" },
            "locale": { "dateFormatted": "Thu Jan 01 1970 01:00:00 GMT+0100" },
            "webrtc": {
                "addresses": ["a1b2.local", "203.0.113.9", "9.9.9.9"],
                "types": "host,srflx",
            },
        });
        let n = scrub_webrtc(&mut doc);

        assert_eq!(
            doc["navigator"]["userAgent"],
            "Mozilla/5.0 (Windows NT 10.0) Chrome/150.0.0.0 Safari/537.36"
        );
        assert_eq!(
            doc["clientHints"]["fullVersionList"],
            "Not;A=Brand/8.0.0.0,Chromium/150.0.7871.187"
        );
        // A clock is three colon-separated groups of hex-looking digits, and an
        // earlier version turned this one into 2001:db8::7.
        assert_eq!(doc["locale"]["dateFormatted"], "Thu Jan 01 1970 01:00:00 GMT+0100");

        // And it still does its job.
        assert_eq!(n, 1, "only the routable address should be replaced: {:?}",
                   doc["webrtc"]["addresses"]);
        // 203.0.113.9 is documentation space and stays as it is: redacting an
        // already-redacted capture must be a no-op.
        assert_eq!(doc["webrtc"]["addresses"][0], "a1b2.local");
        assert_eq!(doc["webrtc"]["addresses"][1], "203.0.113.9");
        assert_eq!(doc["webrtc"]["addresses"][2], "203.0.113.7");
        // The finding survives the redaction, which is the whole point of
        // substituting rather than deleting.
        assert_eq!(doc["webrtc"]["types"], "host,srflx");
    }

    #[test]
    fn private_and_documentation_addresses_are_left_alone() {
        for a in ["192.168.1.4", "10.0.0.7", "127.0.0.1", "169.254.1.1",
                  "100.64.0.1", "203.0.113.7", "203.0.113.9", "192.0.2.5",
                  "198.51.100.9", "fe80::1", "::1", "2001:db8::7"] {
            let mut doc: Value = serde_json::json!({ "webrtc": { "addresses": [a] } });
            assert_eq!(scrub_webrtc(&mut doc), 0, "{a} should not be rewritten");
        }
    }
}

// ---------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Identity,
    Spoof,
}

struct Finding {
    path: String,
    kind: Kind,
    baseline: Option<String>,
    candidate: Option<String>,
}

impl Finding {
    fn fails_in(&self, mode: Mode) -> bool {
        match self.kind {
            Kind::Noise => false,
            Kind::Behavioural => true,
            Kind::Value => mode == Mode::Identity,
        }
    }
}

fn cmd_diff(args: &[String]) -> Result<()> {
    let mut mode = Mode::Spoof;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--mode" => {
                let v = args.get(i + 1).context("--mode needs a value")?;
                mode = match v.as_str() {
                    "identity" => Mode::Identity,
                    "spoof" => Mode::Spoof,
                    other => anyhow::bail!("unknown mode {other:?}; use identity or spoof"),
                };
                i += 2;
            }
            other => {
                positional.push(other.to_string());
                i += 1;
            }
        }
    }

    let (a_path, b_path) = match positional.as_slice() {
        [a, b] => (a.clone(), b.clone()),
        _ => anyhow::bail!("usage: fury-detect diff [--mode spoof|identity] <baseline> <candidate>"),
    };

    let a = load(&a_path)?;
    let b = load(&b_path)?;

    let mut findings = Vec::new();
    let mut all_paths: Vec<&String> = a.keys().chain(b.keys()).collect();
    all_paths.sort();
    all_paths.dedup();

    for path in all_paths {
        let av = a.get(path);
        let bv = b.get(path);
        if av == bv {
            continue;
        }
        findings.push(Finding {
            path: path.clone(),
            kind: classify(path),
            baseline: av.cloned(),
            candidate: bv.cloned(),
        });
    }

    // One root cause must produce one finding. A subtree marked `__timeout` or
    // `__absent` on one side makes every leaf under it look independently
    // missing: a single WebRTC timeout was reported as three separate
    // behavioural failures, which buries real findings under arithmetic.
    let findings = collapse_absent_subtrees(findings);

    // Comparing dumps from different probe versions silently compares different
    // measurements. The canvas hash definition changed between 0.1 and 0.2, so
    // this is not hypothetical.
    let ver_a = a.get("__probeVersion").cloned().unwrap_or_else(|| "?".into());
    let ver_b = b.get("__probeVersion").cloned().unwrap_or_else(|| "?".into());
    if ver_a != ver_b {
        eprintln!(
            "!! probe version mismatch: baseline {ver_a}, candidate {ver_b}\n\
             !! Re-capture both with the same probe before trusting this diff.\n"
        );
    }

    report(&findings, mode, &a_path, &b_path)
}

/// Drop per-leaf absence findings that are explained by an absent or timed-out
/// parent, keeping the one finding that names the actual cause.
fn collapse_absent_subtrees(findings: Vec<Finding>) -> Vec<Finding> {
    let roots: Vec<String> = findings
        .iter()
        .filter(|f| {
            let leaf = f.path.rsplit('.').next().unwrap_or("");
            leaf == "__timeout" || leaf == "__absent"
        })
        .map(|f| {
            f.path
                .rsplit_once('.')
                .map(|(root, _)| root.to_string())
                .unwrap_or_default()
        })
        .collect();

    findings
        .into_iter()
        .filter(|f| {
            let leaf = f.path.rsplit('.').next().unwrap_or("");
            if leaf == "__timeout" || leaf == "__absent" {
                return true;
            }
            // Only absences are explained by an absent parent. A field present
            // on both sides with different values is a finding either way.
            let one_side_missing = f.baseline.is_none() || f.candidate.is_none();
            if !one_side_missing {
                return true;
            }
            !roots
                .iter()
                .any(|r| !r.is_empty() && f.path.starts_with(&format!("{r}.")))
        })
        .collect()
}

/// Some values are enormous — the speechSynthesis voice list alone is ~8 KB, and
/// printing two of them buries every other finding in the report. Show enough to
/// identify the value and say how much was cut; `flatten` prints the whole thing
/// when the detail is actually needed.
fn abbreviate(v: Option<&str>) -> String {
    const LIMIT: usize = 160;
    match v {
        None => "<field absent>".to_string(),
        Some(s) if s.chars().count() <= LIMIT => s.to_string(),
        Some(s) => {
            let head: String = s.chars().take(LIMIT).collect();
            let cut = s.chars().count() - LIMIT;
            format!("{head}… (+{cut} символов)")
        }
    }
}

fn report(findings: &[Finding], mode: Mode, a_path: &str, b_path: &str) -> Result<()> {
    let mode_name = match mode {
        Mode::Identity => "identity (same browser twice — nothing may differ)",
        Mode::Spoof => "spoof (Chrome vs Fury — values may differ, behaviour may not)",
    };

    println!("baseline   {}", Path::new(a_path).display());
    println!("candidate  {}", Path::new(b_path).display());
    println!("mode       {mode_name}\n");

    let mut failures: Vec<&Finding> = findings.iter().filter(|f| f.fails_in(mode)).collect();
    failures.sort_by_key(|f| (f.kind != Kind::Behavioural, f.path.clone()));

    let behavioural = failures.iter().filter(|f| f.kind == Kind::Behavioural).count();
    let values = failures.len() - behavioural;

    if !failures.is_empty() {
        println!("FAILURES");
        println!("────────");
        for f in &failures {
            let label = match f.kind {
                Kind::Behavioural => "behaviour",
                Kind::Value => "value",
                Kind::Noise => "noise",
            };
            println!("  [{label}] {}", f.path);
            println!("      baseline  {}", abbreviate(f.baseline.as_deref()));
            println!("      candidate {}", abbreviate(f.candidate.as_deref()));
        }
        println!();
    }

    if mode == Mode::Spoof {
        let spoofed = findings
            .iter()
            .filter(|f| f.kind == Kind::Value && !f.fails_in(mode))
            .count();
        println!("{spoofed} identity value(s) differ — expected, this is the spoofing working");
    }

    let noise = findings.iter().filter(|f| f.kind == Kind::Noise).count();
    if noise > 0 {
        println!("{noise} noisy field(s) ignored");
    }

    println!();
    if failures.is_empty() {
        println!("PASS — no behavioural differences");
        Ok(())
    } else {
        println!("FAIL — {behavioural} behavioural, {values} value difference(s) that must not differ in this mode");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// gate
// ---------------------------------------------------------------------------

/// Single-dump checks, mirroring the release gate in docs/07-detection-baseline.md.
/// These need no baseline: they are internal contradictions.
fn cmd_gate(args: &[String]) -> Result<()> {
    let path = args.first().context("usage: fury-detect gate <dump.json>")?;
    let flat = load(path)?;
    let get = |k: &str| flat.get(k).map(String::as_str);

    struct Check {
        what: &'static str,
        verdict: Verdict,
        detail: String,
    }
    #[derive(PartialEq)]
    enum Verdict {
        Pass,
        Fail,
        Skip,
    }

    let mut checks = Vec::new();
    let mut add = |what: &'static str, verdict: Verdict, detail: String| {
        checks.push(Check { what, verdict, detail })
    };

    // 1. Cross-context consistency — the headline result.
    match get("crossContext.consistent") {
        Some("true") => add("cross-context consistency", Verdict::Pass, "no disagreements".into()),
        Some(_) => add(
            "cross-context consistency",
            Verdict::Fail,
            format!(
                "{} disagreement(s): {}",
                get("crossContext.disagreementCount").unwrap_or("?"),
                flat.iter()
                    .filter(|(k, _)| k.starts_with("crossContext.disagreements.") && k.ends_with(".field"))
                    .map(|(_, v)| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        None => add("cross-context consistency", Verdict::Skip, "field missing".into()),
    }

    // 2-4. Stability. An unstable spoof is worse than none.
    for (field, label) in [
        ("canvas2d.stableAcrossCalls", "canvas stable across calls"),
        ("webgl.webgl1.render.stableAcrossCalls", "webgl stable across calls"),
        ("audio.stableAcrossCalls", "audio stable across calls"),
    ] {
        match get(field) {
            Some("true") => add(label, Verdict::Pass, "stable".into()),
            Some(v) => add(label, Verdict::Fail, format!("unstable ({v})")),
            None => add(label, Verdict::Skip, "field missing".into()),
        }
    }

    // 5. No JS-injection traces.
    match get("engine.nativeToString") {
        Some(v) if v.contains("PATCHED") => {
            add("getters are native", Verdict::Fail, v.to_string())
        }
        Some(v) => add("getters are native", Verdict::Pass, v.to_string()),
        None => add("getters are native", Verdict::Skip, "field missing".into()),
    }

    // 6. Proprietary codecs — identifies a self-built Chromium instantly.
    match get("codecs.hasProprietaryCodecs") {
        Some("true") => add("H.264 / AAC present", Verdict::Pass, "yes".into()),
        Some(_) => add(
            "H.264 / AAC present",
            Verdict::Fail,
            "missing — set proprietary_codecs=true (docs/03)".into(),
        ),
        None => add("H.264 / AAC present", Verdict::Skip, "field missing".into()),
    }

    // 7. Widevine.
    if flat.contains_key("drm.com\\.widevine\\.alpha.available")
        || flat.keys().any(|k| k.contains("widevine") && k.ends_with("available"))
    {
        add("Widevine available", Verdict::Pass, "yes".into());
    } else if flat.keys().any(|k| k.contains("widevine")) {
        add(
            "Widevine available",
            Verdict::Fail,
            "refused — real Chrome accepts it (docs/10)".into(),
        );
    } else {
        add("Widevine available", Verdict::Skip, "not probed".into());
    }

    // 8. Permissions API self-consistency.
    //
    // Computed here from the raw states rather than reading the probe's
    // precomputed boolean: a gate that trusts a summary field inherits whatever
    // bug produced it. The two APIs name the same states differently —
    // Permissions says `prompt`, Notification says `default` — so normalise
    // before comparing. Real Chrome 150 reports prompt/default.
    // A plain fn, not a closure: a closure returning a borrow of its own
    // argument cannot express the higher-ranked lifetime this needs.
    fn normalise(v: &str) -> &str {
        if v == "default" {
            "prompt"
        } else {
            v
        }
    }
    match (
        get("permissions.notifications"),
        get("permissions.__notificationApi"),
    ) {
        (Some(query), Some(api)) => {
            let detail = format!("query={query} vs Notification.permission={api}");
            if normalise(query) == normalise(api) {
                add("Permissions API self-consistent", Verdict::Pass, detail);
            } else {
                add("Permissions API self-consistent", Verdict::Fail, detail);
            }
        }
        _ => add("Permissions API self-consistent", Verdict::Skip, "field missing".into()),
    }

    // 9. Automation traces.
    let cdc = get("engine.automation.cdcVars").unwrap_or("");
    let doc_cdc = get("engine.automation.documentCdc").unwrap_or("");
    if cdc.is_empty() && doc_cdc.is_empty() {
        add("no ChromeDriver traces", Verdict::Pass, "clean".into());
    } else {
        add("no ChromeDriver traces", Verdict::Fail, format!("{cdc} {doc_cdc}"));
    }

    match get("navigator.webdriver") {
        Some("false") => add("navigator.webdriver === false", Verdict::Pass, "false".into()),
        Some(v) => add(
            "navigator.webdriver === false",
            Verdict::Fail,
            format!("{v} — real Chrome reports exactly false"),
        ),
        None => add("navigator.webdriver === false", Verdict::Skip, "field missing".into()),
    }

    // 10. Scrollbar must match the claimed OS.
    match (get("navigator.platform"), get("screen.scrollbarWidth")) {
        (Some(platform), Some(w)) => {
            let is_mac = platform.contains("Mac");
            let width: i64 = w.parse().unwrap_or(-1);
            let ok = if is_mac { width == 0 } else { width > 0 };
            let detail = format!("{width}px with platform={platform}");
            if ok {
                add("scrollbar matches claimed OS", Verdict::Pass, detail);
            } else {
                add("scrollbar matches claimed OS", Verdict::Fail, detail);
            }
        }
        _ => add("scrollbar matches claimed OS", Verdict::Skip, "field missing".into()),
    }

    // 11. WebGPU must describe the same GPU as WebGL.
    match (get("webgpu.vendor"), get("webgl.webgl1.unmasked.renderer")) {
        (Some(vendor), Some(renderer)) => {
            let ok = renderer.to_lowercase().contains(&vendor.to_lowercase());
            let detail = format!("{vendor} vs {renderer}");
            if ok {
                add("WebGPU agrees with WebGL", Verdict::Pass, detail);
            } else {
                add("WebGPU agrees with WebGL", Verdict::Fail, detail);
            }
        }
        _ => add("WebGPU agrees with WebGL", Verdict::Skip, "WebGPU absent".into()),
    }

    // 12. Timezone must reach workers too.
    match (get("locale.timezone"), get("contexts.worker.timezone")) {
        (Some(main), Some(worker)) => {
            let detail = format!("{main} / {worker}");
            if main == worker {
                add("timezone reaches workers", Verdict::Pass, detail);
            } else {
                add("timezone reaches workers", Verdict::Fail, detail);
            }
        }
        _ => add("timezone reaches workers", Verdict::Skip, "field missing".into()),
    }

    // --- report ---
    println!("gate  {}\n", Path::new(path).display());
    let mut failed = 0;
    for c in &checks {
        let mark = match c.verdict {
            Verdict::Pass => "PASS",
            Verdict::Fail => {
                failed += 1;
                "FAIL"
            }
            Verdict::Skip => "skip",
        };
        println!("  {mark}  {:<34} {}", c.what, c.detail);
    }

    println!();
    if failed == 0 {
        println!("PASS — all gate checks satisfied");
        Ok(())
    } else {
        println!("FAIL — {failed} gate check(s) failed");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flat_of(v: Value) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        flatten(&v, "", &mut out);
        out
    }

    #[test]
    fn flattens_nested_objects_to_dotted_paths() {
        let f = flat_of(json!({ "a": { "b": { "c": 1 } } }));
        assert_eq!(f.get("a.b.c").map(String::as_str), Some("1"));
    }

    #[test]
    fn arrays_become_one_joined_scalar() {
        // Indexed paths make an inserted element look like a diff in every
        // following position.
        let f = flat_of(json!({ "fonts": ["Arial", "Menlo"] }));
        assert_eq!(f.get("fonts").map(String::as_str), Some("Arial,Menlo"));
        assert!(!f.contains_key("fonts.0"));
    }

    #[test]
    fn null_is_a_value_not_an_absence() {
        // Distinguishing "API returned null" from "field not present" matters:
        // the first is a legitimate cross-context absence, the second a schema
        // change.
        let f = flat_of(json!({ "x": null }));
        assert_eq!(f.get("x").map(String::as_str), Some("null"));
    }

    #[test]
    fn behavioural_findings_fail_in_both_modes() {
        let f = Finding {
            path: "codecs.hasProprietaryCodecs".into(),
            kind: Kind::Behavioural,
            baseline: Some("true".into()),
            candidate: Some("false".into()),
        };
        assert!(f.fails_in(Mode::Spoof));
        assert!(f.fails_in(Mode::Identity));
    }

    #[test]
    fn value_findings_fail_only_in_identity_mode() {
        // A different canvas hash is the spoof working, not a bug.
        let f = Finding {
            path: "canvas2d.toDataURL".into(),
            kind: Kind::Value,
            baseline: Some("aaaa".into()),
            candidate: Some("bbbb".into()),
        };
        assert!(!f.fails_in(Mode::Spoof));
        assert!(f.fails_in(Mode::Identity));
    }

    #[test]
    fn noise_never_fails() {
        let f = Finding {
            path: "screen.outerHeight".into(),
            kind: Kind::Noise,
            baseline: Some("900".into()),
            candidate: Some("1080".into()),
        };
        assert!(!f.fails_in(Mode::Spoof));
        assert!(!f.fails_in(Mode::Identity));
    }
}
