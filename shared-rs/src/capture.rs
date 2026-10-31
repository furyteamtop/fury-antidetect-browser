// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Turning a probe capture into a persona.
//!
//! Lives in shared-rs (moved 12.09.2026 from tools/detect-suite) so that the
//! agent can do it too: the desktop's "capture this machine" button and the
//! `fury-detect persona` command are the same conversion, and two copies of
//! the mapping would drift the first time a field was added to one.
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

use serde_json::{json, Value};

/// A string at a dotted path in the capture, e.g. `navigator.userAgent`.
pub fn s(v: &Value, path: &str) -> Option<String> {
    let mut cur = v;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    cur.as_str().map(str::to_string)
}

/// A number at a dotted path in the capture.
pub fn n(v: &Value, path: &str) -> Option<f64> {
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
/// The WebGL parameters a persona carries: the numbers and ranges patch 0031
/// answers from it, which a WebGL1 and a WebGL2 context report alike. The
/// windows-11 and macOS personas, written by hand, carry exactly these.
const WEBGL_PARAMS: &[&str] = &[
    "ALIASED_LINE_WIDTH_RANGE",
    "ALIASED_POINT_SIZE_RANGE",
    "ALPHA_BITS",
    "BLUE_BITS",
    "DEPTH_BITS",
    "GREEN_BITS",
    "MAX_COMBINED_TEXTURE_IMAGE_UNITS",
    "MAX_CUBE_MAP_TEXTURE_SIZE",
    "MAX_FRAGMENT_UNIFORM_VECTORS",
    "MAX_RENDERBUFFER_SIZE",
    "MAX_TEXTURE_IMAGE_UNITS",
    "MAX_TEXTURE_SIZE",
    "MAX_VARYING_VECTORS",
    "MAX_VERTEX_ATTRIBS",
    "MAX_VERTEX_TEXTURE_IMAGE_UNITS",
    "MAX_VERTEX_UNIFORM_VECTORS",
    "MAX_VIEWPORT_DIMS",
    "RED_BITS",
    "SAMPLES",
    "SAMPLE_BUFFERS",
    "STENCIL_BITS",
    "SUBPIXEL_BITS",
];

pub fn from_capture(dump: &Value, id: &str, weight: f64) -> Result<Value, String> {
    // A capture from a browser that is already spoofing describes a
    // description. Fury sets automation.hideTraces, so webdriver reads false
    // while the debugger is attached — that is not the tell. The tell is the
    // brand list: an unbranded Chromium says Chromium and not Google Chrome, and
    // a Fury profile says whatever its persona told it to.
    if dump.get("__fury").is_some() {
        return Err(format!("this capture came from Fury; a persona must come from an unmodified browser"));
    }

    let ua = s(dump, "navigator.userAgent").ok_or("capture has no navigator.userAgent")?;
    let platform = s(dump, "navigator.platform").ok_or("capture has no navigator.platform")?;
    let ch_platform = s(dump, "clientHints.platform").unwrap_or_else(|| {
        if platform.starts_with("Win") {
            "Windows".into()
        } else if ua.contains("Android") {
            "Android".into()
        } else {
            "macOS".into()
        }
    });

    let renderer = s(dump, "webgl.webgl2.unmasked.renderer")
        .or_else(|| s(dump, "webgl.webgl1.unmasked.renderer"))
        .ok_or(
            "capture has no unmasked WebGL renderer — the GPU is the single most \
             identifying field in a persona and one cannot be built without it",
            )?;
    let vendor = s(dump, "webgl.webgl2.unmasked.vendor")
        .or_else(|| s(dump, "webgl.webgl1.unmasked.vendor"))
        .ok_or("capture has no unmasked WebGL vendor")?;

    // Consistency, checked here rather than left to persona.validate() so the
    // message names the capture rather than the file it produced.
    let mac = ch_platform == "macOS";
    let android = ch_platform == "Android";
    if android && (renderer.contains("Direct3D") || renderer.contains("Metal")) {
        return Err(format!("capture claims Android with a desktop renderer: {renderer}"));
    }
    if mac && (renderer.contains("Direct3D") || renderer.contains("D3D11")) {
        return Err(format!("capture claims macOS with a Direct3D renderer: {renderer}"));
    }
    if !mac && renderer.contains("Metal") {
        return Err(format!("capture claims {ch_platform} with a Metal renderer: {renderer}"));
    }

    let fonts: Vec<String> = dump
        .get("fonts")
        .and_then(|f| f.get("detectedByMeasurement"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    if fonts.is_empty() {
        return Err(format!(
            "capture has no fonts.detectedByMeasurement — patch 0050 narrows the \
             fallback list to the persona's, and an empty list would ask for a \
             machine with no fonts"
        ));
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

    // Only what patch 0031 answers from the persona AND reads the same in a
    // WebGL1 and a WebGL2 context. The core applies one map to both, so a
    // string like VERSION, taken from the webgl2 block, made every WebGL1
    // context call itself "WebGL 2.0" -- a pair no real browser produces.
    // Found 14.10.2026 measuring the first phone capture against the core;
    // the GTX 950 persona had carried it since 13.09. VENDOR and RENDERER are
    // Blink constants the persona writes itself, and the webgl2-only limits
    // are not read at all.
    let webgl_params: serde_json::Map<String, Value> = ["webgl2", "webgl1"]
        .iter()
        .find_map(|ctx| dump.get("webgl")?.get(ctx)?.get("params")?.as_object())
        .map(|params| {
            params
                .iter()
                .filter(|(k, v)| WEBGL_PARAMS.contains(&k.as_str()) && !v.is_null())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        })
        .unwrap_or_default();
    let extensions_of = |ctx: &str| -> Vec<String> {
        dump.get("webgl")
            .and_then(|w| w.get(ctx))
            .and_then(|w| w.get("extensions"))
            .and_then(|v| v.as_str())
            .map(|s| s.split(',').filter(|e| !e.is_empty()).map(str::to_string).collect())
            .unwrap_or_default()
    };
    // Each context's own list, and the WebGL2 one again as the fallback an
    // older core reads for both.
    let webgl1_extensions = extensions_of("webgl1");
    let webgl2_extensions = extensions_of("webgl2");
    let webgl_extensions =
        if webgl2_extensions.is_empty() { webgl1_extensions.clone() } else { webgl2_extensions.clone() };

    // A phone GPU's shader precision is its own (persona.rs, shader_precision);
    // a desktop's is the one every desktop answers, so the host's is right and
    // nothing is carried. The WebGL1 block: both captured phones answer the
    // same in WebGL2.
    let shader_precision: serde_json::Map<String, Value> = if android {
        dump.get("webgl")
            .and_then(|w| w.get("webgl1"))
            .and_then(|w| w.get("precision"))
            .and_then(|p| p.as_object())
            .map(|p| p.iter().filter(|(_, v)| v.is_string()).map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    } else {
        Default::default()
    };

    // The WebGPU adapter as the machine reported it. Without this block the
    // core answers with the HOST's adapter: an Android persona on a Mac said
    // "apple / metal-3" beside a Qualcomm WebGL renderer. The features narrow
    // the host's set (patch 0032), so a feature the real device lacked is not
    // offered either.
    let webgpu = dump
        .get("webgpu")
        .filter(|w| w.get("vendor").and_then(|v| v.as_str()).is_some_and(|v| !v.is_empty()))
        .map(|w| {
            let text = |k: &str| w.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let limits: serde_json::Map<String, Value> = w
                .get("limits")
                .and_then(|l| l.as_object())
                .map(|l| l.iter().filter(|(_, v)| v.is_number()).map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default();
            let features: Vec<String> = text("features")
                .split(',')
                .filter(|f| !f.is_empty())
                .map(str::to_string)
                .collect();
            json!({
                "vendor": text("vendor"),
                "architecture": text("architecture"),
                "device": text("device"),
                "description": text("description"),
                "limits": limits,
                "features": features,
            })
        });

    // A phone carries what a desktop has no field for. Android Chrome leaves
    // architecture and bitness empty in the hints, and says "Mobile" in the
    // user agent exactly when it is a phone.
    let mobile = android.then(|| {
        json!({
            "model": s(dump, "clientHints.model").unwrap_or_default(),
            "form_factor": if ua.contains(" Mobile ") { "phone" } else { "tablet" },
            "connection": s(dump, "engine.misc.connectionKind")
                .filter(|k| k == "wifi" || k == "cellular")
                .unwrap_or_else(|| "cellular".into()),
            "voices": android_voice_locales(dump),
        })
    });
    let (arch_default, bitness_default) = if android { ("", "") } else { ("x86", "64") };

    // The release, not platformVersion: on Windows that is 7.0.0 for a
    // Windows 10 machine (see PersonaOs::display_version).
    let platform_version = s(dump, "clientHints.platformVersion").unwrap_or_default();
    let os_version = match crate::persona::windows_release(&platform_version) {
        Some(release) if !mac && !android => release.to_string(),
        _ => platform_version,
    };

    let mut persona = json!({
        "id": id,
        "weight": weight,
        // NOT "measured". See the module comment.
        "source": "capture",
        "os": {
            "name": if mac { "macOS" } else if android { "Android" } else { "Windows" },
            "version": os_version,
            "arch": if android { "arm64".to_string() } else { s(dump, "clientHints.architecture").unwrap_or_else(|| "x86".into()) },
            "user_agent_template": ua_template(&ua),
            "platform": platform,
            "ch_platform": ch_platform,
            "ch_platform_version": s(dump, "clientHints.platformVersion").unwrap_or_default(),
            "ch_architecture": s(dump, "clientHints.architecture").unwrap_or_else(|| arch_default.into()),
            "ch_bitness": s(dump, "clientHints.bitness").unwrap_or_else(|| bitness_default.into()),
        },
        "gpu": {
            "webgl_vendor": vendor,
            "webgl_renderer": renderer,
            "webgl_params": webgl_params,
            "webgl_extensions": webgl_extensions,
            "webgl1_extensions": webgl1_extensions,
            "webgl2_extensions": webgl2_extensions,
            "shader_precision": shader_precision,
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
    });
    // Absent rather than empty, as the persona itself writes them.
    if let Some(gpu) = persona["gpu"].as_object_mut() {
        gpu.retain(|k, v| {
            !matches!(k.as_str(), "webgl1_extensions" | "webgl2_extensions" | "shader_precision")
                || v.as_array().is_some_and(|a| !a.is_empty())
                || v.as_object().is_some_and(|o| !o.is_empty())
        });
    }
    if let Some(g) = webgpu {
        persona["gpu"]["webgpu"] = g;
    }
    if let Some(m) = mobile {
        persona["mobile"] = m;
        // The status bar, reserved on some handsets and not on others.
        if let Some(top) = n(dump, "screen.availTop") {
            persona["screen"]["avail_top"] = json!(top as u32);
        }
    }
    Ok(persona)
}

/// A phone's voice locales, in the order Chrome on Android lists them.
///
/// The probe sorts voices by name; Chrome lists one per locale in the order
/// `Locale.getAvailableLocales()` returns them, which is ICU's, sorted by the
/// locale id: language, then script, then country, with the current ISO codes
/// where Java still writes the withdrawn ones (in, iw, ji). The first is the
/// default voice, and on both captured phones that is "as_IN", which this
/// order puts first.
fn android_voice_locales(dump: &Value) -> Vec<String> {
    let mut locales: Vec<String> = s(dump, "speech.names")
        .map(|joined| {
            joined
                .split('~')
                .filter_map(|entry| entry.split('|').nth(1).map(str::to_string))
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let icu_id = |java: &str| -> String {
        let mut parts = java.split('_');
        let language = match parts.next().unwrap_or_default() {
            "in" => "id",
            "iw" => "he",
            "ji" => "yi",
            other => other,
        }
        .to_string();
        let mut country = String::new();
        let mut script = String::new();
        for part in parts {
            if let Some(sc) = part.strip_prefix('#') {
                script = sc.to_string();
            } else if country.is_empty() {
                country = part.to_string();
            }
        }
        [language, script, country].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_")
    };
    locales.sort_by_key(|l| icu_id(l));
    locales.dedup();
    locales
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first phone capture, converted. Measured against the core it
    /// showed two things the converter got wrong for every persona built
    /// from a capture (14.10.2026): the webgl2 block's VERSION went into the
    /// map the core answers BOTH contexts from, and the WebGPU adapter was
    /// left out, so the host's answered.
    #[test]
    fn a_phone_capture_keeps_its_gpu_and_only_the_shared_webgl_numbers() {
        let dump: Value = serde_json::from_str(include_str!(
            "../../tools/detect-suite/baselines/chrome-155-android-realme-rmx5131-redacted.json"
        ))
        .unwrap();
        let p = from_capture(&dump, "android-test", 0.02).unwrap();
        let params = p["gpu"]["webgl_params"].as_object().unwrap();
        for absent in ["VERSION", "SHADING_LANGUAGE_VERSION", "VENDOR", "RENDERER", "MAX_SAMPLES"] {
            assert!(!params.contains_key(absent), "{absent} would be answered in both contexts");
        }
        assert!(params.keys().all(|k| WEBGL_PARAMS.contains(&k.as_str())));
        assert_eq!(params["MAX_TEXTURE_SIZE"], 8192);
        assert_eq!(params.len(), WEBGL_PARAMS.len(), "the phone reported every one of them");

        let gpu = &p["gpu"]["webgpu"];
        assert_eq!(gpu["vendor"], "qualcomm");
        assert_eq!(gpu["architecture"], "adreno-7xx");
        assert_eq!(gpu["limits"]["maxStorageBufferBindingSize"], 134217728);
        assert!(gpu["features"].as_array().unwrap().iter().any(|f| f == "texture-compression-astc"));

        let persona: crate::Persona = serde_json::from_value(p).unwrap();
        persona.validate().unwrap();
        assert!(persona.is_phone());
    }

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
