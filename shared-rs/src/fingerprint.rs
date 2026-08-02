// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Fingerprint configuration: the JSON handed to the patched Chromium core.
//!
//! Two rules govern everything here:
//!
//! 1. **Determinism.** `(persona, seed) -> config` is a pure function. The same
//!    profile must produce a byte-identical config on every launch and on every
//!    machine, or the site sees one identity running on changing hardware.
//! 2. **Consistency.** Every value must be mutually plausible. A valid but
//!    impossible combination (macOS claiming an NVIDIA D3D11 renderer) is worse
//!    than no spoofing at all — it is a positive signal rather than a neutral one.
//!
//! `validate()` enforces rule 2 and refuses the launch. It is deliberately
//! fail-closed: an inconsistent profile is a burned account.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintConfig {
    pub schema_version: u32,
    pub os: OsSpec,
    pub navigator: NavigatorSpec,
    pub screen: ScreenSpec,
    pub gpu: GpuSpec,
    pub audio: AudioSpec,
    pub fonts: Vec<String>,
    pub locale: LocaleSpec,
    pub media_devices: Vec<MediaDeviceSpec>,
    pub webrtc: WebRtcSpec,
    pub noise: NoiseSpec,
    pub network: NetworkSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsSpec {
    /// "Windows" | "macOS"
    pub name: String,
    pub version: String,
    /// "x86_64" | "arm64"
    pub arch: String,
    pub bitness: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigatorSpec {
    pub user_agent: String,
    pub platform: String,
    pub hardware_concurrency: u32,
    pub device_memory_gb: u32,
    pub max_touch_points: u32,
    pub languages: Vec<String>,
    pub pdf_viewer_enabled: bool,
    /// Client Hints. These MUST be the single source of truth shared by the
    /// HTTP headers and `navigator.userAgentData.getHighEntropyValues()`.
    /// Deriving them twice is the most common way products get caught.
    pub ua_full_version_list: Vec<(String, String)>,
    pub ua_platform_version: String,
    pub ua_model: String,
    pub ua_mobile: bool,
    pub ua_wow64: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenSpec {
    pub width: u32,
    pub height: u32,
    pub avail_width: u32,
    pub avail_height: u32,
    pub color_depth: u32,
    pub device_pixel_ratio: f64,
    /// outerHeight - innerHeight for a default window. Platform-specific and
    /// routinely measured; getting it wrong contradicts the claimed OS.
    pub chrome_height_delta: u32,
    /// 0 on macOS (overlay scrollbars), ~15-17 on Windows.
    pub scrollbar_width: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuSpec {
    pub webgl_vendor: String,
    pub webgl_renderer: String,
    /// All ~80 `getParameter` constants for this GPU.
    pub webgl_params: serde_json::Map<String, serde_json::Value>,
    pub webgl_extensions: Vec<String>,
    /// WebGPU adapter info and limits. Must describe the same physical GPU as
    /// the WebGL fields above — the mismatch is a ready-made detection vector
    /// that most commercial products still leave open.
    pub webgpu: Option<WebGpuSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebGpuSpec {
    pub vendor: String,
    pub architecture: String,
    pub device: String,
    pub description: String,
    pub limits: serde_json::Map<String, serde_json::Value>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSpec {
    pub sample_rate: u32,
    pub base_latency: f64,
    pub output_latency: f64,
    pub voices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocaleSpec {
    /// IANA name, e.g. "Europe/Amsterdam".
    pub timezone: String,
    pub locale: String,
    pub geo: Option<GeoSpec>,
    /// Must match the locale — a ru-RU profile reporting a US keyboard layout
    /// through `navigator.keyboard.getLayoutMap()` is a contradiction.
    pub keyboard_layout: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoSpec {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaDeviceSpec {
    /// "audioinput" | "audiooutput" | "videoinput"
    pub kind: String,
    pub device_id: String,
    pub group_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebRtcMode {
    /// No WebRTC at all. Safest, but its absence is itself unusual.
    Disabled,
    /// Host candidates replaced with a plausible private address, srflx with
    /// the proxy IP. Default.
    Fake,
    /// Real WebRTC, but forced through the proxy.
    ProxyOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtcSpec {
    pub mode: WebRtcMode,
    pub public_ip: Option<String>,
    pub local_ip: Option<String>,
}

/// Deterministic per-profile noise seeds. Derived from the profile seed so that
/// two launches of the same profile produce identical canvas/WebGL/audio
/// readbacks. Detectors call `toDataURL()` twice and compare.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseSpec {
    pub canvas_seed: u64,
    pub webgl_seed: u64,
    pub audio_seed: u64,
    pub client_rects_seed: u64,
    /// Salt for `mediaDevices.enumerateDevices()` device IDs.
    pub device_id_salt: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSpec {
    /// Name of the TLS profile in `shared/tls-profiles/` to replay.
    pub tls_profile: String,
    /// HTTP/2 SETTINGS values in wire order.
    pub h2_settings: Vec<(u16, u32)>,
    pub h2_window_update: u32,
    pub header_order: Vec<String>,
    pub enable_quic: bool,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ConsistencyError {
    #[error("{0}")]
    Inconsistent(String),
}

impl FingerprintConfig {
    /// Fail-closed consistency check. Every rule here corresponds to a check
    /// that public fingerprint auditors (CreepJS, Pixelscan) actually perform.
    pub fn validate(&self) -> Result<(), Vec<ConsistencyError>> {
        let mut errs = Vec::new();
        let mut bad = |m: String| errs.push(ConsistencyError::Inconsistent(m));

        let is_mac = self.os.name == "macOS";
        let is_win = self.os.name == "Windows";

        // --- OS vs GPU driver stack -------------------------------------
        let r = &self.gpu.webgl_renderer;
        if is_mac && (r.contains("Direct3D") || r.contains("D3D11") || r.contains("vs_5_0")) {
            bad(format!("macOS profile reports a Direct3D renderer: {r}"));
        }
        if is_win && r.contains("Metal") {
            bad(format!("Windows profile reports a Metal renderer: {r}"));
        }
        if is_mac && (r.contains("NVIDIA") || r.contains("AMD Radeon RX")) {
            bad(format!(
                "macOS profile reports a GPU Apple has not shipped in years: {r}"
            ));
        }

        // --- WebGL vs WebGPU describe the same device -------------------
        if let Some(wg) = &self.gpu.webgpu {
            let rl = r.to_lowercase();
            let vendor_in_webgl = rl.contains(&wg.vendor.to_lowercase());
            if !vendor_in_webgl {
                bad(format!(
                    "WebGPU vendor '{}' does not appear in the WebGL renderer '{}'",
                    wg.vendor, r
                ));
            }
        }

        // --- OS vs platform strings -------------------------------------
        let expected_platform = if is_mac { "MacIntel" } else { "Win32" };
        if self.navigator.platform != expected_platform {
            bad(format!(
                "navigator.platform '{}' does not match OS '{}' (expected '{}')",
                self.navigator.platform, self.os.name, expected_platform
            ));
        }

        // --- OS vs scrollbar --------------------------------------------
        if is_mac && self.screen.scrollbar_width != 0 {
            bad("macOS uses overlay scrollbars; scrollbar_width must be 0".into());
        }
        if is_win && self.screen.scrollbar_width == 0 {
            bad("Windows always reserves scrollbar width; 0 contradicts the OS".into());
        }

        // --- Touch points ------------------------------------------------
        if is_mac && self.navigator.max_touch_points != 0 {
            bad("macOS reports maxTouchPoints = 0; no Mac has a touchscreen".into());
        }

        // --- Memory vs cores --------------------------------------------
        if self.navigator.device_memory_gb < 4 && self.navigator.hardware_concurrency > 8 {
            bad(format!(
                "{} GB RAM with {} cores is not a machine that exists",
                self.navigator.device_memory_gb, self.navigator.hardware_concurrency
            ));
        }
        // deviceMemory must be a power of two. The spec describes clamping to
        // 8, but measurement beats the spec: real Chrome 150 on a 16 GB Mac
        // reports 16. The original rule here was `[1,2,4,8]`, which would have
        // made the launcher refuse a persona copied from a genuine browser.
        let mem = self.navigator.device_memory_gb;
        if mem == 0 || (mem & (mem - 1)) != 0 || mem > 64 {
            bad(format!(
                "navigator.deviceMemory should be a power of two (observed in the \
                 wild: 4, 8, 16); got {mem}"
            ));
        }

        // --- Screen ------------------------------------------------------
        if self.screen.avail_width > self.screen.width
            || self.screen.avail_height > self.screen.height
        {
            bad("availWidth/availHeight cannot exceed the screen size".into());
        }
        if is_mac && self.screen.avail_height == self.screen.height {
            bad("macOS always reserves the menu bar; availHeight must be < height".into());
        }

        // --- Locale vs timezone ------------------------------------------
        if let Some(country) = self.locale.locale.split('-').nth(1) {
            if !timezone_plausible_for_country(&self.locale.timezone, country) {
                bad(format!(
                    "timezone '{}' is implausible for locale '{}'",
                    self.locale.timezone, self.locale.locale
                ));
            }
        }

        // --- Geo vs timezone ---------------------------------------------
        if let Some(geo) = &self.locale.geo {
            if !(-90.0..=90.0).contains(&geo.latitude)
                || !(-180.0..=180.0).contains(&geo.longitude)
            {
                bad("geo coordinates out of range".into());
            }
        }

        // --- Fonts vs OS --------------------------------------------------
        // The default font sets barely overlap; a wrong set gives the OS away
        // through measurement even if the enumeration API is filtered.
        if is_mac && self.fonts.iter().any(|f| f == "Bahnschrift" || f == "Segoe UI Variable") {
            bad("Windows-only fonts present on a macOS profile".into());
        }
        if is_win && self.fonts.iter().any(|f| f == "Helvetica Neue" || f == "SF Pro") {
            bad("macOS-only fonts present on a Windows profile".into());
        }
        if self.fonts.is_empty() {
            bad("empty font list is itself a fingerprint".into());
        }

        // --- User agent vs claimed OS --------------------------------------
        let ua = &self.navigator.user_agent;
        if is_mac && !ua.contains("Macintosh") {
            bad("user agent does not mention Macintosh on a macOS profile".into());
        }
        if is_win && !ua.contains("Windows NT") {
            bad("user agent does not mention Windows NT on a Windows profile".into());
        }
        if self.navigator.ua_mobile {
            bad("desktop profile must report Sec-CH-UA-Mobile: ?0".into());
        }

        // --- Client Hints vs user agent -------------------------------------
        // Both must come from the same source; if a major version appears in one
        // and not the other, that alone is a detection.
        if let Some((_, chrome_ver)) = self
            .navigator
            .ua_full_version_list
            .iter()
            .find(|(brand, _)| brand == "Google Chrome")
        {
            let major = chrome_ver.split('.').next().unwrap_or_default();
            if !major.is_empty() && !ua.contains(&format!("Chrome/{major}.")) {
                bad(format!(
                    "Client Hints claim Chrome {chrome_ver} but the user agent disagrees"
                ));
            }
        }

        // --- Audio ----------------------------------------------------------
        if ![44100, 48000].contains(&self.audio.sample_rate) {
            bad(format!(
                "unusual AudioContext sampleRate {}; real machines report 44100 or 48000",
                self.audio.sample_rate
            ));
        }

        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}

/// Coarse plausibility check. A real implementation loads the IANA zone-to-country
/// mapping; this is the shape of the rule, not the full table.
fn timezone_plausible_for_country(tz: &str, country: &str) -> bool {
    let prefix = match country {
        "US" | "CA" => "America/",
        "RU" => "Europe/",
        "GB" | "DE" | "FR" | "NL" | "PL" | "ES" | "IT" => "Europe/",
        "BR" | "AR" | "MX" => "America/",
        "JP" | "CN" | "IN" | "SG" | "KR" => "Asia/",
        "AU" | "NZ" => "Australia/",
        _ => return true,
    };
    // Russia spans Asia too; treat both as acceptable.
    tz.starts_with(prefix) || (country == "RU" && tz.starts_with("Asia/"))
}

/// Hand-written reference configs.
///
/// Not test fixtures — the agent, the detect suite and the persona validator all
/// need a known-good starting point, and having exactly one of them means a
/// consistency rule cannot be satisfied in tests but violated in production.
pub mod samples {
    use super::*;

    /// A consistent macOS / Apple Silicon persona.
    pub fn macos_arm64() -> FingerprintConfig {
        FingerprintConfig {
            schema_version: 1,
            os: OsSpec {
                name: "macOS".into(),
                version: "15.2".into(),
                arch: "arm64".into(),
                bitness: "64".into(),
            },
            navigator: NavigatorSpec {
                user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                             AppleWebKit/537.36 (KHTML, like Gecko) \
                             Chrome/151.0.0.0 Safari/537.36"
                    .into(),
                platform: "MacIntel".into(),
                hardware_concurrency: 10,
                device_memory_gb: 8,
                max_touch_points: 0,
                languages: vec!["en-US".into(), "en".into()],
                pdf_viewer_enabled: true,
                ua_full_version_list: vec![("Google Chrome".into(), "151.0.7842.60".into())],
                ua_platform_version: "15.2.0".into(),
                ua_model: String::new(),
                ua_mobile: false,
                ua_wow64: false,
            },
            screen: ScreenSpec {
                width: 1728,
                height: 1117,
                avail_width: 1728,
                avail_height: 1080,
                color_depth: 30,
                device_pixel_ratio: 2.0,
                chrome_height_delta: 87,
                scrollbar_width: 0,
            },
            gpu: GpuSpec {
                webgl_vendor: "Google Inc. (Apple)".into(),
                webgl_renderer: "ANGLE (Apple, ANGLE Metal Renderer: Apple M3 Pro, Unspecified Version)"
                    .into(),
                webgl_params: Default::default(),
                webgl_extensions: vec!["EXT_color_buffer_float".into()],
                webgpu: Some(WebGpuSpec {
                    vendor: "apple".into(),
                    architecture: "common-3".into(),
                    device: String::new(),
                    description: String::new(),
                    limits: Default::default(),
                    features: vec![],
                }),
            },
            audio: AudioSpec {
                sample_rate: 48000,
                base_latency: 0.0106,
                output_latency: 0.02,
                voices: vec!["Samantha".into()],
            },
            fonts: vec!["Helvetica Neue".into(), "Menlo".into()],
            locale: LocaleSpec {
                timezone: "America/New_York".into(),
                locale: "en-US".into(),
                geo: None,
                keyboard_layout: "us".into(),
            },
            media_devices: vec![],
            webrtc: WebRtcSpec {
                mode: WebRtcMode::Fake,
                public_ip: None,
                local_ip: Some("192.168.1.34".into()),
            },
            noise: NoiseSpec {
                canvas_seed: 1,
                webgl_seed: 2,
                audio_seed: 3,
                client_rects_seed: 4,
                device_id_salt: 5,
            },
            network: NetworkSpec {
                tls_profile: "chrome-151-macos".into(),
                h2_settings: vec![(1, 65536), (2, 0), (4, 6291456), (6, 262144)],
                h2_window_update: 15663105,
                header_order: vec![":method".into(), ":authority".into()],
                enable_quic: false,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// the core's wire contract
// ---------------------------------------------------------------------------

/// Every config path the patched core actually reads.
///
/// Not hand-kept. `core_config_keys_match_the_patches` scans
/// `core/patches/*.patch` for the read sites and fails if this list has drifted
/// from them, so adding a patch that reads a new key breaks the build until the
/// key is both listed here and supplied by `derive_core_config`.
///
/// The list existed by hand before, and by hand it was wrong: eleven of the
/// keys below were missing, which is why a Windows persona once answered with
/// the host's macOS speech voices. The failure is silent by construction — the
/// core looks a path up, does not find it, and falls back to stock Chromium for
/// that one vector. Nothing logs and nothing crashes; the profile reports the
/// real machine while looking configured.
///
/// A trailing dot means a prefix rather than a leaf: the core builds the path at
/// runtime (`GetInt(std::string("mediaDevices.") + key)`), so what has to exist
/// is a non-empty object there, not one named key.
pub const CORE_CONFIG_KEYS: &[&str] = &[
    "audio.outputLatency",
    "automation.hideTraces",
    "battery.charging",
    "battery.chargingTime",
    "battery.dischargingTime",
    "battery.level",
    "clientHints.formFactors",
    "clientHints.mobile",
    "clientHints.wow64",
    "engine.jsHeapSizeLimit",
    "fonts",
    "geolocation.accuracy",
    "geolocation.latitude",
    "geolocation.longitude",
    "gpu.webglExtensions",
    "gpu.webglParams.",
    "gpu.webgpu.architecture",
    "gpu.webgpu.description",
    "gpu.webgpu.device",
    "gpu.webgpu.features",
    "gpu.webgpu.limits.",
    "gpu.webgpu.vendor",
    "locale.locale",
    "locale.timezone",
    "mediaDevices.",
    "navigator.deviceMemory",
    "navigator.hardwareConcurrency",
    "navigator.languages",
    "navigator.maxTouchPoints",
    "navigator.platform",
    "navigator.userAgent",
    "noise.audioSeed",
    "noise.canvasSeed",
    "noise.clientRectsSeed",
    "permissions.",
    "permissions.notifications",
    "screen.availLeft",
    "screen.availTop",
    "screen.chromeHeightDelta",
    "screen.chromeWidthDelta",
    "screen.colorDepth",
    "screen.devicePixelRatio",
    "screen.scrollbarWidth",
    "speech.voices",
    "webrtc.ipHandlingPolicy",
];

/// Keys whose whole branch is optional, and the branch that decides.
///
/// `gpu.webgpu` is absent on a persona from a machine with no WebGPU adapter,
/// which is a real machine, not a misconfigured one.
///
/// `speech` and `mediaDevices` are absent when the persona lists no voices or
/// no devices. Their patches narrow a real list to the persona's, so an empty
/// persona would ask for zero of everything — see the note in
/// `derive_core_config`. Absent means that vector falls through to stock
/// Chromium: a leak, tracked as persona-data work, and a smaller lie than a
/// browser with no speaker.
///
/// `geolocation` is absent when the exit checker could not place the proxy —
/// and absent is the right answer there, not a fallback coordinate. Patch 0082
/// creates no provider without it, so the machine's own location service
/// answers, exactly as it would in a browser with no Fury in it. Inventing a
/// position from the country alone would drop the profile in the middle of a
/// country while its address resolves to a city, which is worse than not
/// knowing: geolocation needs a permission prompt to be read at all, and the
/// site that bothers to ask is the site that checks it against the IP.
const OPTIONAL_BRANCHES: &[(&str, &[&str])] = &[
    (
        "gpu.webgpu",
        &[
            "gpu.webgpu.architecture",
            "gpu.webgpu.description",
            "gpu.webgpu.device",
            "gpu.webgpu.features",
            "gpu.webgpu.limits.",
            "gpu.webgpu.vendor",
        ],
    ),
    ("speech", &["speech.voices"]),
    ("mediaDevices", &["mediaDevices."]),
    (
        "geolocation",
        &[
            "geolocation.accuracy",
            "geolocation.latitude",
            "geolocation.longitude",
        ],
    ),
];

/// Checks that a core config carries everything the core will look for.
///
/// Deliberately *not* a consistency check — that is the persona's job, and a
/// config derived from a valid persona cannot contradict itself. This answers a
/// different question: will the core understand what it is handed? Passing it
/// the wrong shape entirely (`FingerprintConfig`, whose keys are snake_case)
/// fails every lookup, and this is what turns that into a refusal to launch
/// rather than a browser that quietly reports the real machine.
pub fn check_core_config(config: &serde_json::Value) -> Result<(), Vec<String>> {
    // A key inside a branch the config does not carry at all is not missing —
    // the whole branch was a decision. A key inside a branch that IS there and
    // is half-filled is missing, and that is the case worth catching.
    let skipped: Vec<&str> = OPTIONAL_BRANCHES
        .iter()
        .filter(|(branch, _)| lookup(config, branch).is_none())
        .flat_map(|(_, keys)| keys.iter().copied())
        .collect();

    let missing: Vec<String> = CORE_CONFIG_KEYS
        .iter()
        .filter(|path| !skipped.contains(path))
        .filter(|path| match path.strip_suffix('.') {
            // A prefix the core completes at runtime: what must be there is an
            // object with something in it. An empty one reads exactly like a
            // missing one from inside the core, so it fails the same way.
            Some(prefix) => !lookup(config, prefix).is_some_and(|v| {
                v.as_object().is_some_and(|o| !o.is_empty())
            }),
            None => lookup(config, path).is_none(),
        })
        .map(|path| format!("the core reads {path}, and this config has no such key"))
        .collect();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Dotted-path lookup, matching the core's own `FuryConfig::Get*`.
fn lookup<'a>(config: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    path.split('.')
        .try_fold(config, |node, part| node.get(part))
        .filter(|v| !v.is_null())
}

#[cfg(test)]
mod tests {
    use super::samples::macos_arm64 as mac_config;

    #[test]
    fn a_consistent_mac_profile_passes() {
        mac_config().validate().expect("should be consistent");
    }

    #[test]
    fn nvidia_on_macos_is_rejected() {
        let mut c = mac_config();
        c.gpu.webgl_renderer =
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 vs_5_0 ps_5_0, D3D11)".into();
        c.gpu.webgpu = None;
        assert!(c.validate().is_err());
    }

    #[test]
    fn windows_scrollbar_on_macos_is_rejected() {
        let mut c = mac_config();
        c.screen.scrollbar_width = 15;
        assert!(c.validate().is_err());
    }

    #[test]
    fn client_hints_must_agree_with_user_agent() {
        let mut c = mac_config();
        c.navigator.ua_full_version_list = vec![("Google Chrome".into(), "149.0.7100.20".into())];
        assert!(c.validate().is_err());
    }

    #[test]
    fn impossible_hardware_is_rejected() {
        let mut c = mac_config();
        c.navigator.device_memory_gb = 2;
        c.navigator.hardware_concurrency = 24;
        assert!(c.validate().is_err());
    }
}
