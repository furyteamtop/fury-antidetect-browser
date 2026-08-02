// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Turning a probe capture into a persona.
//!
//! # Why this exists
//!
//! The catalogue has fourteen machines. Fourteen is a thin crowd to hide in, and
//! the README says adding one is the most useful thing an outside contributor
//! can do — but until now there was no way to do it. The probe produced a
//! capture and nothing turned a capture into a persona, so the only way to add a
//! machine was to hand-write four hundred lines of JSON and hope it was
//! internally consistent.
//!
//! The alternative to this tool is inventing personas, and inventing them is
//! precisely what this project criticises the competition for. shared-rs's own
//! module comment names it: fingerprint-chromium derives hardwareConcurrency as
//! `((seed % 13) + 4) * 2` and hardcodes deviceMemory to 8, so it will report a
//! 32-core machine with 8 GB of RAM — a machine that does not exist, and a
//! stronger signal than no spoofing at all. Every value in a persona has to come
//! off one real machine, together. A capture is exactly that: one machine,
//! measured, all at once.
//!
//! # What it will not do
//!
//! It refuses a capture from a browser that is already spoofing. A persona built
//! from a Fury capture would describe a description, and the error is worth more
//! than the file.
//!
//! It marks the result `source: "capture"` and not `"measured"`. `measured`
//! means somebody dumped values off physical hardware and checked them;
//! `capture` means a browser reported them. The distinction is in the catalogue
//! already — see the note at catalogue.rs:320-330 — and diluting the stronger
//! word would make it useless.

use anyhow::{Context, Result};
use serde_json::{json, Value};

fn s(v: &Value, path: &str) -> Option<String> {
    let mut cur = v;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    cur.as_str().map(str::to_string)
}

fn n(v: &Value, path: &str) -> Option<f64> {
    let mut cur = v;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    cur.as_f64()
}

/// The user agent with this build's version replaced by a placeholder.
///
/// A persona outlives a Chromium uprev; a user agent with 150.0.0.0 baked in
/// does not. `derive_core_config` substitutes {CHROME_MAJOR} at launch, so the
/// template is what gets stored — otherwise every persona in the catalogue would
/// need rewriting on every upgrade, which is how a catalogue stops being
/// updated.
fn ua_template(ua: &str) -> String {
    // Chrome/150.0.0.0 -> Chrome/{CHROME_MAJOR}.0.0.0
    let mut out = String::with_capacity(ua.len() + 8);
    let mut rest = ua;
    while let Some(at) = rest.find("Chrome/") {
        out.push_str(&rest[..at + 7]);
        rest = &rest[at + 7..];
        let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        out.push_str("{CHROME_MAJOR}");
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Build a persona from a capture, or say why the capture cannot produce one.
pub fn from_capture(dump: &Value, id: &str, weight: f64) -> Result<Value> {
    // A capture from a browser that is already spoofing describes a
    // description. Fury sets automation.hideTraces, so webdriver reads false
    // while the debugger is attached — that is not the tell. The tell is the
    // brand list: an unbranded Chromium says Chromium and not Google Chrome, and
    // a Fury profile says whatever its persona told it to.
    if dump.get("__fury").is_some() {
        anyhow::bail!("this capture came from Fury; a persona must come from an unmodified browser");
    }

    let ua = s(dump, "navigator.userAgent").context("capture has no navigator.userAgent")?;
    let platform = s(dump, "navigator.platform").context("capture has no navigator.platform")?;
    let ch_platform = s(dump, "clientHints.platform").unwrap_or_else(|| {
        if platform.starts_with("Win") { "Windows".into() } else { "macOS".into() }
    });

    let renderer = s(dump, "webgl.webgl2.unmasked.renderer")
        .or_else(|| s(dump, "webgl.webgl1.unmasked.renderer"))
        .context(
            "capture has no unmasked WebGL renderer — the GPU is the single most \
             identifying field in a persona and one cannot be built without it",
        )?;
    let vendor = s(dump, "webgl.webgl2.unmasked.vendor")
        .or_else(|| s(dump, "webgl.webgl1.unmasked.vendor"))
        .context("capture has no unmasked WebGL vendor")?;

    // Consistency, checked here rather than left to persona.validate() so the
    // message names the capture rather than the file it produced.
    let mac = ch_platform == "macOS";
    if mac && (renderer.contains("Direct3D") || renderer.contains("D3D11")) {
        anyhow::bail!("capture claims macOS with a Direct3D renderer: {renderer}");
    }
    if !mac && renderer.contains("Metal") {
        anyhow::bail!("capture claims {ch_platform} with a Metal renderer: {renderer}");
    }

    let fonts: Vec<String> = dump
        .get("fonts")
        .and_then(|f| f.get("detectedByMeasurement"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if fonts.is_empty() {
        anyhow::bail!(
            "capture has no fonts.detectedByMeasurement — patch 0050 narrows the \
             fallback list to the persona's, and an empty list would ask for a \
             machine with no fonts"
        );
    }

    // Voices are "Name|lang|localService~Name|lang|..." in the capture and a
    // plain list of names in a persona.
    let voices: Vec<String> = s(dump, "speech.names")
        .map(|joined| {
            joined
                .split('~')
                .filter_map(|entry| entry.split('|').next().map(str::to_string))
                .filter(|v| !v.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let media_devices: Vec<Value> = s(dump, "mediaDevices.kinds")
        .map(|kinds| {
            kinds.split(',').filter(|k| !k.is_empty()).map(|k| json!({ "kind": k })).collect()
        })
        .unwrap_or_default();

    let webgl_params = dump
        .get("webgl")
        .and_then(|w| w.get("webgl2"))
        .and_then(|w| w.get("params"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let webgl_extensions: Vec<String> = dump
        .get("webgl")
        .and_then(|w| w.get("webgl2"))
        .and_then(|w| w.get("extensions"))
        .and_then(|v| v.as_str())
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();

    Ok(json!({
        "id": id,
        "weight": weight,
        // NOT "measured". See the module comment.
        "source": "capture",
        "os": {
            "name": if mac { "macOS" } else { "Windows" },
            "version": s(dump, "clientHints.platformVersion").unwrap_or_default(),
            "arch": s(dump, "clientHints.architecture").unwrap_or_else(|| "x86".into()),
            "user_agent_template": ua_template(&ua),
            "platform": platform,
            "ch_platform": ch_platform,
            "ch_platform_version": s(dump, "clientHints.platformVersion").unwrap_or_default(),
            "ch_architecture": s(dump, "clientHints.architecture").unwrap_or_else(|| "x86".into()),
            "ch_bitness": s(dump, "clientHints.bitness").unwrap_or_else(|| "64".into()),
        },
        "gpu": {
            "webgl_vendor": vendor,
            "webgl_renderer": renderer,
            "webgl_params": webgl_params,
            "webgl_extensions": webgl_extensions,
        },
        "screen": {
            "width": n(dump, "screen.width").unwrap_or(0.0) as u32,
            "height": n(dump, "screen.height").unwrap_or(0.0) as u32,
            "avail_width": n(dump, "screen.availWidth").unwrap_or(0.0) as u32,
            "avail_height": n(dump, "screen.availHeight").unwrap_or(0.0) as u32,
            "color_depth": n(dump, "screen.colorDepth").unwrap_or(24.0) as u32,
            "device_pixel_ratio": n(dump, "screen.devicePixelRatio").unwrap_or(1.0),
        },
        "chrome_metrics": {
            // The OS tell, and the reason a persona cannot be assembled from
            // specifications: this is the height of the browser's own chrome and
            // it differs between platforms and versions. Phase 0 measured 143 px
            // on a clean build against 87 on real Chrome.
            "outer_minus_inner_height": n(dump, "screen.chromeHeight").unwrap_or(0.0) as u32,
            "outer_minus_inner_width": n(dump, "screen.chromeWidth").unwrap_or(0.0) as u32,
            "scrollbar_width": n(dump, "screen.scrollbarWidth").unwrap_or(0.0) as u32,
        },
        "cpu": { "cores": n(dump, "navigator.hardwareConcurrency").unwrap_or(8.0) as u32 },
        "memory_gb": n(dump, "navigator.deviceMemory").unwrap_or(8.0) as u32,
        "max_touch_points": n(dump, "navigator.maxTouchPoints").unwrap_or(0.0) as u32,
        "fonts": fonts,
        "audio": {
            "sample_rate": n(dump, "audio.context.sampleRate").unwrap_or(48000.0) as u32,
            "base_latency": n(dump, "audio.context.baseLatency").unwrap_or(0.0),
            "output_latency": n(dump, "audio.context.outputLatency").unwrap_or(0.0),
        },
        "voices": voices,
        "media_devices": media_devices,
    }))
}

pub fn cmd_persona(args: &[String]) -> Result<()> {
    let mut input = None;
    let mut id = None;
    let mut weight = 0.01_f64;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => {
                id = args.get(i + 1).cloned();
                i += 2;
            }
            "--weight" => {
                weight = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(weight);
                i += 2;
            }
            other => {
                input = Some(other.to_string());
                i += 1;
            }
        }
    }
    let input = input.context(
        "usage: fury-detect persona <capture.json> [--id name] [--weight 0.01]",
    )?;

    let dump: Value = serde_json::from_str(&std::fs::read_to_string(&input)?)?;

    // A default id that describes the machine rather than the file, because a
    // catalogue of persona-1, persona-2 is a catalogue nobody can reason about.
    let default_id = {
        let plat = s(&dump, "clientHints.platform").unwrap_or_else(|| "unknown".into());
        let ver = s(&dump, "clientHints.platformVersion").unwrap_or_default();
        let gpu = s(&dump, "webgl.webgl2.unmasked.renderer").unwrap_or_default();
        let w = n(&dump, "screen.width").unwrap_or(0.0) as u32;
        let h = n(&dump, "screen.height").unwrap_or(0.0) as u32;
        // Pull the model out of the renderer string rather than the whole
        // clause: "ANGLE Metal Renderer: Apple M5" makes an id nobody can read,
        // and an id nobody can read is a catalogue nobody can reason about.
        let short_gpu: String = {
            let flat = gpu.replace(&[',', '(', ')', ':'][..], " ");
            let words: Vec<&str> = flat.split_whitespace().collect();
            let at = words.iter().position(|w| {
                ["RTX", "GTX", "Radeon", "Iris", "UHD"].contains(w)
                    || (*w == "M5" || *w == "M4" || *w == "M3" || *w == "M2" || *w == "M1")
            });
            match at {
                Some(i) => {
                    // The model plus what follows it, up to two words: "RTX 4060",
                    // "Apple M5", "Iris Xe".
                    let start = if words[i].starts_with('M') && i > 0 { i - 1 } else { i };
                    words[start..(start + 2).min(words.len())].join("-").to_lowercase()
                }
                None => "gpu".to_string(),
            }
        };
        format!(
            "{}-{}-{}-{}x{}",
            plat.to_lowercase().replace(' ', ""),
            ver.split('.').next().unwrap_or("0"),
            short_gpu,
            w,
            h
        )
    };
    let id = id.unwrap_or(default_id);

    let persona = from_capture(&dump, &id, weight)?;

    // Refuse to emit a persona the launcher would refuse to launch. Catching it
    // here costs a line; catching it in the field costs an account.
    let parsed: fury_shared::Persona = serde_json::from_value(persona.clone())
        .context("the persona built from this capture does not match the schema")?;
    parsed
        .validate()
        .map_err(|errs| anyhow::anyhow!(
            "the persona built from this capture is internally inconsistent:\n  {}",
            errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n  ")
        ))?;

    println!("{}", serde_json::to_string_pretty(&persona)?);
    eprintln!(
        "\n  {id}\n  {} fonts, {} voices, {} media devices\n  \
         source: \"capture\" — a browser reported these. \"measured\" is reserved \
         for values taken off physical hardware and checked.\n",
        parsed.fonts.len(),
        parsed.voices.len(),
        parsed.media_devices.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_agent_becomes_a_template() {
        // A persona outlives a Chromium uprev. One with 150 baked into it does
        // not, and a catalogue that must be rewritten on every upgrade is a
        // catalogue that stops being updated.
        assert_eq!(
            ua_template("Mozilla/5.0 (Windows NT 10.0) Chrome/150.0.0.0 Safari/537.36"),
            "Mozilla/5.0 (Windows NT 10.0) Chrome/{CHROME_MAJOR}.0.0.0 Safari/537.36"
        );
        // No Chrome token: unchanged rather than mangled.
        assert_eq!(ua_template("Mozilla/5.0 Safari/537.36"), "Mozilla/5.0 Safari/537.36");
    }

    #[test]
    fn a_capture_that_contradicts_itself_is_refused() {
        let dump = json!({
            "navigator": { "userAgent": "Chrome/150.0.0.0", "platform": "MacIntel" },
            "clientHints": { "platform": "macOS" },
            "webgl": { "webgl2": { "unmasked": {
                "renderer": "ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 vs_5_0, D3D11)",
                "vendor": "Google Inc. (NVIDIA)" } } },
            "fonts": { "detectedByMeasurement": ["Arial"] },
        });
        let err = from_capture(&dump, "x", 0.01).unwrap_err().to_string();
        assert!(err.contains("Direct3D"), "{err}");
    }

    #[test]
    fn a_capture_with_no_fonts_is_refused_rather_than_emptied() {
        // Patch 0050 narrows the fallback list to the persona's. An empty list
        // asks for a machine with no fonts, which is not a machine.
        let dump = json!({
            "navigator": { "userAgent": "Chrome/150.0.0.0", "platform": "Win32" },
            "clientHints": { "platform": "Windows" },
            "webgl": { "webgl2": { "unmasked": {
                "renderer": "ANGLE (Intel, Intel(R) UHD Graphics Direct3D11 vs_5_0, D3D11)",
                "vendor": "Google Inc. (Intel)" } } },
        });
        let err = from_capture(&dump, "x", 0.01).unwrap_err().to_string();
        assert!(err.contains("fonts"), "{err}");
    }
}
