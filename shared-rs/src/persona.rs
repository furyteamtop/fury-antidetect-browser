//! Device personas, and deriving a core config from one.
//!
//! A persona is a real machine's measured configuration. A profile picks one
//! and supplies a seed; everything the core is told is then derived from that
//! pair, deterministically.
//!
//! This exists because of what measurement showed about the alternative.
//! fingerprint-chromium derives values independently from the seed —
//! `hardwareConcurrency` as `((seed % 13) + 4) * 2` and `deviceMemory` as a
//! hardcoded `return 8` — so it can emit 32 cores with 8 GB of RAM, a machine
//! that does not exist. A valid-but-impossible combination is a stronger signal
//! than no spoofing at all, because it is positive evidence of tampering rather
//! than merely an absence of protection.
//!
//! So: values that belong together travel together, in one persona, taken from
//! one real machine.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One real machine's measured configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Persona {
    pub id: String,
    /// Share of the real population. Personas below 0.005 are never
    /// auto-assigned: a rare-but-real configuration still narrows the crowd.
    pub weight: f64,
    /// Where the numbers came from. `measured` means someone dumped them off a
    /// physical machine.
    #[serde(default)]
    pub source: Option<String>,
    pub os: PersonaOs,
    pub gpu: PersonaGpu,
    pub screen: PersonaScreen,
    pub chrome_metrics: PersonaChromeMetrics,
    pub cpu: PersonaCpu,
    /// Spec-quantised, and measurement beats the spec: real Chrome 150 on a
    /// 16 GB Mac reports 16, not the 8 the spec text implies.
    pub memory_gb: u32,
    #[serde(default)]
    pub max_touch_points: u32,
    pub fonts: Vec<String>,
    pub audio: PersonaAudio,
    #[serde(default)]
    pub voices: Vec<String>,
    #[serde(default)]
    pub media_devices: Vec<PersonaMediaDevice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaOs {
    /// "Windows" | "macOS"
    pub name: String,
    pub version: String,
    /// "x86_64" | "arm64"
    pub arch: String,
    /// The UA string template, with {CHROME_MAJOR} substituted at derive time
    /// so a persona survives a Chromium uprev without being rewritten.
    pub user_agent_template: String,
    /// navigator.platform: "Win32" or "MacIntel".
    pub platform: String,
    /// Sec-CH-UA-Platform.
    pub ch_platform: String,
    pub ch_platform_version: String,
    pub ch_architecture: String,
    pub ch_bitness: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaGpu {
    pub webgl_vendor: String,
    pub webgl_renderer: String,
    /// Numbers and comma-joined ranges, keyed by the GL constant name.
    #[serde(default)]
    pub webgl_params: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub webgl_extensions: Vec<String>,
    #[serde(default)]
    pub webgpu: Option<PersonaWebGpu>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaWebGpu {
    pub vendor: String,
    pub architecture: String,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub limits: BTreeMap<String, f64>,
    /// Personas may only NARROW the host's feature set — see patch 0032. A
    /// feature listed here that the host GPU lacks is a validation error, not
    /// something the core can invent.
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaScreen {
    pub width: u32,
    pub height: u32,
    pub avail_width: u32,
    pub avail_height: u32,
    pub color_depth: u32,
    pub device_pixel_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaChromeMetrics {
    /// outerHeight - innerHeight. Differs between Windows and macOS and is
    /// routinely measured.
    pub outer_minus_inner_height: u32,
    #[serde(default)]
    pub outer_minus_inner_width: u32,
    /// 0 on macOS (overlay scrollbars), 15-17 on Windows.
    pub scrollbar_width: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaCpu {
    pub cores: u32,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaAudio {
    pub sample_rate: u32,
    pub base_latency: f64,
    pub output_latency: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaMediaDevice {
    /// "audioinput" | "audiooutput" | "videoinput"
    pub kind: String,
}

/// Per-profile inputs that are not part of the persona: they come from the
/// proxy, not from the device.
#[derive(Debug, Clone)]
pub struct ProfileContext {
    /// IANA zone derived from the proxy exit IP. See docs/05 — a profile
    /// exiting in Germany while reporting Asia/Tbilisi is the cheapest
    /// detection in the industry.
    pub timezone: String,
    /// BCP-47 list, most preferred first.
    pub languages: Vec<String>,
    /// The UI locale the core runs as — one of Chrome's shipped ones.
    ///
    /// Not a second opinion about `languages`: it is derived from them by
    /// `locale::ui_locale_for`, computed once by the caller, and used twice —
    /// here, and as `--lang` on the core's command line. `--lang` is what
    /// actually makes `Intl` agree, because it is the lever Chrome itself uses;
    /// this copy exists so the config is a faithful record of what the profile
    /// claims rather than a partial one.
    ///
    /// Measured: tools/detect-suite/baselines/final.json is a Fury capture with
    /// `navigator.languages = en-US,en`, a Windows persona and America/New_York
    /// — reporting `locale.locale = ru` and formatting numbers `123 456,789`,
    /// because the core inherited the developer's Mac locale. Timezone and
    /// languages were both already right. This is the field that was missing.
    pub ui_locale: String,
    /// Chromium major version this build ships.
    pub chrome_major: u32,
    /// Full four-part version, for the high-entropy Client Hints.
    pub chrome_full_version: String,
}

// ---------------------------------------------------------------------------
// derivation
// ---------------------------------------------------------------------------

/// SplitMix64. Stateless, so the same inputs always give the same output — the
/// property the whole design rests on.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Derives an independent sub-seed for one purpose from the profile seed.
///
/// Separate streams per purpose so that adding a new noised vector later does
/// not shift the canvas fingerprint of every existing profile — which would
/// silently re-identify every account a user has already aged.
fn sub_seed(seed: u64, purpose: &str) -> u32 {
    let mut h = seed;
    for byte in purpose.as_bytes() {
        h = mix(h ^ u64::from(*byte));
    }
    (mix(h) & 0x7fff_ffff) as u32
}

impl Persona {
    /// Builds the JSON the patched core reads.
    ///
    /// Key paths match what components/fury reads and what
    /// tools/detect-suite/probe.js dumps, so a capture from a real machine can
    /// be turned into a persona and back without translation.
    pub fn derive_core_config(&self, seed: u64, ctx: &ProfileContext) -> serde_json::Value {
        let ua = self
            .os
            .user_agent_template
            .replace("{CHROME_MAJOR}", &ctx.chrome_major.to_string());

        let brands = vec![
            "Not;A=Brand/8".to_string(),
            format!("Chromium/{}", ctx.chrome_major),
            format!("Google Chrome/{}", ctx.chrome_major),
        ];
        let full_version_list = vec![
            "Not;A=Brand/8.0.0.0".to_string(),
            format!("Chromium/{}", ctx.chrome_full_version),
            format!("Google Chrome/{}", ctx.chrome_full_version),
        ];

        let mut config = serde_json::json!({
            "schema_version": crate::FINGERPRINT_SCHEMA_VERSION,
            "navigator": {
                "userAgent": ua,
                "platform": self.os.platform,
                "languages": ctx.languages,
                "hardwareConcurrency": self.cpu.cores,
                "deviceMemory": self.memory_gb,
                "maxTouchPoints": self.max_touch_points,
            },
            "clientHints": {
                "brands": brands,
                "fullVersionList": full_version_list,
                "platform": self.os.ch_platform,
                "platformVersion": self.os.ch_platform_version,
                "architecture": self.os.ch_architecture,
                "bitness": self.os.ch_bitness,
                "model": "",
                "mobile": false,
                "wow64": false,
                "fullVersion": ctx.chrome_full_version,
                // Empty is what a desktop Chrome sends. The header exists and
                // is read; omitting the key left the patch falling back to
                // whatever the host reports, which on a phone-shaped host
                // would contradict Sec-CH-UA-Mobile: ?0.
                "formFactors": Vec::<String>::new(),
            },
            "screen": {
                "width": self.screen.width,
                "height": self.screen.height,
                "availWidth": self.screen.avail_width,
                "availHeight": self.screen.avail_height,
                // The menu bar. Zero on Windows, 33 on macOS at the default
                // scale — and a Windows persona reporting a 33-pixel offset, or
                // a macOS one reporting none, is a free contradiction against
                // the platform it just claimed.
                "availLeft": 0,
                "availTop": if self.os.name == "macOS" { 33 } else { 0 },
                "colorDepth": self.screen.color_depth,
                "devicePixelRatio": self.screen.device_pixel_ratio,
                "chromeHeightDelta": self.chrome_metrics.outer_minus_inner_height,
                "chromeWidthDelta": self.chrome_metrics.outer_minus_inner_width,
                "scrollbarWidth": self.chrome_metrics.scrollbar_width,
            },
            "gpu": {
                "webglParams": {},
                "webglExtensions": self.gpu.webgl_extensions,
            },
            "audio": {
                "sampleRate": self.audio.sample_rate,
                "baseLatency": self.audio.base_latency,
                "outputLatency": self.audio.output_latency,
            },
            "fonts": self.fonts,
            "locale": {
                "timezone": ctx.timezone,
                // The UI locale, not the first language tag. They differ: a
                // German profile announces `de-DE` and Intl resolves `de`,
                // which is what real Chrome reports and what this used to get
                // wrong.
                "locale": ctx.ui_locale,
            },
            // Independent streams per vector: adding a new noised surface later
            // must not shift the canvas fingerprint of existing profiles.
            "noise": {
                "canvasSeed": sub_seed(seed, "canvas"),
                "audioSeed": sub_seed(seed, "audio"),
                "clientRectsSeed": sub_seed(seed, "clientRects"),
                "deviceIdSalt": sub_seed(seed, "deviceId"),
            },
        });

        // WebGL params, with the vendor/renderer strings folded into the same
        // map the core reads so there is exactly one place to look.
        let params = config["gpu"]["webglParams"].as_object_mut().unwrap();
        for (key, value) in &self.gpu.webgl_params {
            params.insert(key.clone(), value.clone());
        }
        // VENDOR and RENDERER are Blink constants, not driver strings.
        //
        // Every real Chrome answers "WebKit" and "WebKit WebGL" here, on every
        // GPU and both platforms — verified against the captures in
        // tools/detect-suite/baselines: real Chrome, clean Chromium and even a
        // competitor all report exactly this. Writing the persona's driver
        // string into them made the pair equal to UNMASKED_*, which no real
        // browser ever produces. That is not a fingerprint that failed to
        // spoof; it is positive evidence of tampering, on a zero-false-positive
        // equality check that every commercial collector runs.
        //
        // The driver strings belong in the UNMASKED_* pair and nowhere else.
        params.insert("VENDOR".into(), WEBGL_VENDOR_CONSTANT.into());
        params.insert("RENDERER".into(), WEBGL_RENDERER_CONSTANT.into());
        params.insert(
            "UNMASKED_VENDOR_WEBGL".into(),
            self.gpu.webgl_vendor.clone().into(),
        );
        params.insert(
            "UNMASKED_RENDERER_WEBGL".into(),
            self.gpu.webgl_renderer.clone().into(),
        );

        // Everything below is read by a patch that is written, applied and
        // compiled in, and was reaching a core that had nothing to read.
        //
        // A key the core reads and nobody supplies is not a smaller problem
        // than a wrong value: the patch falls through to stock Chromium, so the
        // vector reports the real machine while the profile looks configured.
        // That is how a Windows persona came to answer with the host's macOS
        // speech voices. See `core_config_keys_match_the_patches`, which reads
        // the list out of the patches instead of trusting a hand-kept copy.
        // Both of these are supplied only when the persona has the data, and
        // that is not squeamishness. Each patch NARROWS a real list to the
        // persona's, so deriving from an empty persona asks for zero voices and
        // zero devices — and no desktop has ever reported either. 0041 already
        // refuses a filter that matches nothing, for that reason; 0060 has no
        // such guard and would obey, leaving a browser with no microphone, no
        // camera and no speaker. A profile that leaks the host's device count is
        // in worse company than one that cannot exist at all.
        //
        // So an empty persona leaves the vector on stock Chromium — the leak
        // this audit set out to close — and closing it is a data job, not a code
        // one: `shared/personas/*.json` ship with `voices: []` and
        // `media_devices: []` and need capturing on real machines.
        if !self.voices.is_empty() {
            config["speech"] = serde_json::json!({ "voices": self.voices });
        }

        if !self.media_devices.is_empty() {
            let count = |kind: &str| {
                self.media_devices.iter().filter(|d| d.kind == kind).count()
            };
            config["mediaDevices"] = serde_json::json!({
                "audioInputCount": count("audioinput"),
                "audioOutputCount": count("audiooutput"),
                "videoInputCount": count("videoinput"),
            });
        }

        // What a real Chrome reports on a profile nobody has answered a prompt
        // in. "prompt" rather than "denied": a site that asks and is refused
        // instantly, every time, on a browser with no history of refusing, is
        // its own signal.
        config["permissions"] = serde_json::json!({
            "notifications": "prompt",
            "geolocation": "prompt",
        });

        // The heap ceiling V8 reports. Not a free number and not a constant:
        // V8 picks it from physical memory at startup and lands on one of a few
        // values, so every desktop from 8 GB up genuinely reports the same
        // 4294705152. What must not happen is a persona claiming 2 GB while
        // reporting the 16 GB ceiling — see `js_heap_limit` for the tiers.
        config["engine"] = serde_json::json!({
            "jsHeapSizeLimit": js_heap_limit(self.memory_gb),
        });

        // The traces an automated Chrome leaves that a driven-but-hidden one
        // should not. On unconditionally: a profile that wants to be driven
        // says so with `cdp`, and that is a different decision.
        config["automation"] = serde_json::json!({ "hideTraces": true });

        if let Some(webgpu) = &self.gpu.webgpu {
            config["gpu"]["webgpu"] = serde_json::json!({
                "vendor": webgpu.vendor,
                "architecture": webgpu.architecture,
                "device": webgpu.device,
                "description": webgpu.description,
                "limits": webgpu.limits,
                "features": webgpu.features,
            });
        }

        config
    }

    /// Internal consistency of the persona itself, before any profile is
    /// derived from it. Catches a bad persona at authoring time rather than
    /// when an account gets banned.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        let is_mac = self.os.name == "macOS";
        let is_win = self.os.name == "Windows";

        if !is_mac && !is_win {
            errs.push(format!("unknown os.name {:?}", self.os.name));
        }

        let expected_platform = if is_mac { "MacIntel" } else { "Win32" };
        if self.os.platform != expected_platform {
            errs.push(format!(
                "os.platform {:?} does not match os.name {:?} (expected {expected_platform:?})",
                self.os.platform, self.os.name
            ));
        }

        if is_mac && self.chrome_metrics.scrollbar_width != 0 {
            errs.push("macOS uses overlay scrollbars; scrollbar_width must be 0".into());
        }
        if is_win && self.chrome_metrics.scrollbar_width == 0 {
            errs.push("Windows always reserves scrollbar width".into());
        }
        if is_mac && self.max_touch_points != 0 {
            errs.push("no Mac has a touchscreen; max_touch_points must be 0".into());
        }

        let r = &self.gpu.webgl_renderer;
        if is_mac && (r.contains("Direct3D") || r.contains("D3D11")) {
            errs.push(format!("macOS persona with a Direct3D renderer: {r}"));
        }
        if is_win && r.contains("Metal") {
            errs.push(format!("Windows persona with a Metal renderer: {r}"));
        }

        if let Some(webgpu) = &self.gpu.webgpu {
            if !r.to_lowercase().contains(&webgpu.vendor.to_lowercase()) {
                errs.push(format!(
                    "WebGPU vendor {:?} does not appear in the WebGL renderer {r:?}",
                    webgpu.vendor
                ));
            }
        }

        if self.screen.avail_width > self.screen.width
            || self.screen.avail_height > self.screen.height
        {
            errs.push("available area cannot exceed the screen".into());
        }
        if is_mac && self.screen.avail_height == self.screen.height {
            errs.push("macOS always reserves the menu bar; avail_height must be < height".into());
        }

        if self.memory_gb == 0 || (self.memory_gb & (self.memory_gb - 1)) != 0 {
            errs.push(format!(
                "memory_gb must be a power of two (4, 8, 16 observed in the wild); got {}",
                self.memory_gb
            ));
        }
        if self.memory_gb < 4 && self.cpu.cores > 8 {
            errs.push(format!(
                "{} GB with {} cores is not a machine that exists",
                self.memory_gb, self.cpu.cores
            ));
        }

        if ![44100, 48000].contains(&self.audio.sample_rate) {
            errs.push(format!(
                "unusual sample rate {}; real machines report 44100 or 48000",
                self.audio.sample_rate
            ));
        }

        if self.fonts.is_empty() {
            errs.push("an empty font list is itself a fingerprint".into());
        }
        if is_mac && self.fonts.iter().any(|f| f == "Segoe UI" || f == "Bahnschrift") {
            errs.push("Windows-only fonts on a macOS persona".into());
        }
        if is_win && self.fonts.iter().any(|f| f == "Helvetica Neue" || f == "Menlo") {
            errs.push("macOS-only fonts on a Windows persona".into());
        }

        if !(0.0..=1.0).contains(&self.weight) {
            errs.push(format!("weight {} is not a share", self.weight));
        }

        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }
}

/// The fingerprint seed, in the one representation everything agrees on.
///
/// Three places hold this value and each had its own idea of it: the server
/// stores `BYTEA`, the local store an `i64`, and the agent hands
/// `derive_core_config` a `u64`. That is fine until a profile crosses between
/// them — and then a seed that round-trips differently is not a smaller
/// problem than a wrong password. It is a different machine. Every launch
/// would present a new fingerprint for an account that has spent months
/// building one, which is the single failure this whole product exists to
/// prevent.
///
/// So it is pinned here, in the crate all three depend on: **eight bytes, big
/// endian, sixteen lowercase hex characters on the wire.**
/// V8's reported heap ceiling for a machine of this size.
///
/// Chrome does not scale this linearly with RAM: it steps, and above 16 GB it
/// stops growing. The values are what desktop Chrome 150 reports on machines of
/// each size, so a persona claiming 8 GB answers what an 8 GB machine answers.
fn js_heap_limit(memory_gb: u32) -> u64 {
    match memory_gb {
        0..=2 => 1_073_741_824,
        3..=4 => 2_197_815_296,
        _ => 4_294_705_152,
    }
}

/// What `gl.getParameter(gl.VENDOR)` returns in every real Chrome.
pub const WEBGL_VENDOR_CONSTANT: &str = "WebKit";
/// And `gl.getParameter(gl.RENDERER)`.
pub const WEBGL_RENDERER_CONSTANT: &str = "WebKit WebGL";

pub mod seed {
    /// The wire form: sixteen lowercase hex characters.
    pub fn to_hex(seed: u64) -> String {
        format!("{seed:016x}")
    }

    /// Parse the wire form. `None` for anything that is not exactly sixteen
    /// hex characters — a shorter string would parse into a different number
    /// and silently give the profile a different machine.
    pub fn from_hex(s: &str) -> Option<u64> {
        if s.len() != 16 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u64::from_str_radix(s, 16).ok()
    }

    /// The storage form for a database with a bytes column.
    pub fn to_bytes(seed: u64) -> [u8; 8] {
        seed.to_be_bytes()
    }

    pub fn from_bytes(b: &[u8]) -> Option<u64> {
        Some(u64::from_be_bytes(b.try_into().ok()?))
    }

    /// The local store keeps an `i64`, because SQLite has no unsigned integer.
    /// Same eight bytes, reinterpreted — never a numeric conversion, which
    /// would saturate or panic on half the range.
    pub fn from_i64(v: i64) -> u64 {
        v as u64
    }

    pub fn to_i64(v: u64) -> i64 {
        v as i64
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_seed_survives_every_representation_it_passes_through() {
        // The one that matters: a seed with the top bit set. As an i64 it is
        // negative, and a numeric conversion rather than a reinterpretation
        // would lose it — giving a warmed account a different machine.
        for seed in [0u64, 1, 0x0123_4567_89ab_cdef, u64::MAX, 1 << 63] {
            assert_eq!(seed::from_hex(&seed::to_hex(seed)), Some(seed));
            assert_eq!(seed::from_bytes(&seed::to_bytes(seed)), Some(seed));
            assert_eq!(seed::from_i64(seed::to_i64(seed)), seed);
            assert_eq!(seed::to_hex(seed).len(), 16, "the wire form is fixed width");
        }
    }

    #[test]
    fn a_seed_that_is_not_exactly_right_is_refused() {
        // Truncated, over-long, or not hex. Each of these would otherwise
        // parse into some number, and some number is a different machine.
        for bad in ["", "0", "123456789abcdef", "0123456789abcdef0", "0123456789abcdeg", " 123456789abcdef"] {
            assert_eq!(seed::from_hex(bad), None, "accepted {bad:?}");
        }
        assert_eq!(seed::from_bytes(&[0; 7]), None);
        assert_eq!(seed::from_bytes(&[0; 9]), None);
    }

    use super::*;

    fn load(name: &str) -> Persona {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../shared/personas/");
        let text = std::fs::read_to_string(format!("{path}{name}.json"))
            .unwrap_or_else(|e| panic!("reading persona {name}: {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parsing persona {name}: {e}"))
    }

    fn ctx() -> ProfileContext {
        ProfileContext {
            timezone: "America/New_York".into(),
            languages: vec!["en-US".into(), "en".into()],
            ui_locale: "en-US".into(),
            chrome_major: 150,
            chrome_full_version: "150.0.7871.187".into(),
        }
    }

    #[test]
    fn shipped_personas_are_internally_consistent() {
        for name in ["macos-15-m-series-1728x1117", "windows-11-rtx4060-1920x1080"] {
            load(name)
                .validate()
                .unwrap_or_else(|e| panic!("persona {name}: {e:?}"));
        }
    }

    #[test]
    fn core_config_covers_every_key_the_core_reads() {
        // The test that would have caught the launcher sending the wrong shape.
        // A derived config must satisfy every path harvested from the core
        // patches; if a new patch reads a new key, add it to CORE_CONFIG_KEYS
        // and this fails until the derivation supplies it.
        for name in ["macos-15-m-series-1728x1117", "windows-11-rtx4060-1920x1080"] {
            let derived = load(name).derive_core_config(1, &ctx());
            crate::fingerprint::check_core_config(&derived)
                .unwrap_or_else(|missing| panic!("persona {name}:\n  {}", missing.join("\n  ")));
        }
    }

    /// Every key the patches read, read out of the patches.
    ///
    /// The point of doing it here rather than by eye: the hand-kept list was
    /// wrong for eleven keys and nobody noticed, because nothing fails when a
    /// key is missing — the core just falls back to stock Chromium for that one
    /// vector. A list maintained by a person tracks what the person remembered;
    /// this one tracks what the C++ actually asks for.
    ///
    /// Three read shapes appear in the patches and all three are found here:
    ///
    ///   config->GetString("navigator.platform")          — a plain key
    ///   apply("permissions.notifications", …)            — through a helper
    ///   GetInt(std::string("mediaDevices.") + key)       — a runtime prefix
    ///
    /// Only added lines (`+`) count: a key that a patch *removes* from stock
    /// Chromium is not a key the built core reads.
    fn keys_the_patches_read() -> std::collections::BTreeSet<String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("core/patches");

        let mut keys = std::collections::BTreeSet::new();
        let mut patches = 0usize;

        for entry in std::fs::read_dir(&dir).expect("core/patches must exist") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("patch") {
                continue;
            }
            patches += 1;
            let text = std::fs::read_to_string(&path).unwrap_or_default();

            // Joined into one line first. The reads are wrapped by clang-format
            // often enough that a line-at-a-time scan silently misses them —
            // `GetStringList(` and `"speech.voices"` sit on different lines —
            // and a scanner that under-reports is worse than none, because it
            // reports success.
            let added: String = text
                .lines()
                .filter(|l| l.starts_with('+'))
                .map(|l| &l[1..])
                .collect::<Vec<_>>()
                .join(" ");

            let mut rest = added.as_str();
            while let Some(at) = rest.find('"') {
                let after = &rest[at + 1..];
                let Some(close) = after.find('"') else { break };
                let literal = &after[..close];
                let before = &rest[..at];

                // A read is a quote preceded by `(`, optional whitespace, and
                // a call whose name ends in one of these. `std::string("x.")`
                // counts too, and keeps its trailing dot as the prefix marker.
                let head = before.trim_end().trim_end_matches('(').trim_end();
                let is_read = ["GetString", "GetStringList", "GetInt", "GetDouble", "GetBool",
                               "GetList", "GetDict", "apply"]
                    .iter()
                    .any(|f| head.ends_with(f))
                    && before.trim_end().ends_with('(');
                let is_prefix = head.ends_with("std::string")
                    && before.trim_end().ends_with('(')
                    && literal.ends_with('.')
                    && after[close + 1..].trim_start().starts_with(')');

                // Skipped: `"gpu.webgpu.limits." #name` is the stringifying
                // macro, whose halves are separate tokens. The prefix is picked
                // up from its `std::string(...)` form in the same patch.
                if (is_read || is_prefix) && !literal.is_empty() {
                    keys.insert(literal.to_string());
                }
                rest = &after[close + 1..];
            }
        }

        // A floor, not a count. If this ever reads zero patches it would pass
        // by finding nothing to disagree with, which is the one way a scanner
        // like this fails silently.
        assert!(patches >= 15, "only {patches} patches scanned — wrong directory?");
        assert!(!keys.is_empty(), "scanned {patches} patches and found no config reads");
        keys
    }

    #[test]
    fn core_config_keys_match_the_patches() {
        let read = keys_the_patches_read();
        let listed: std::collections::BTreeSet<String> = crate::fingerprint::CORE_CONFIG_KEYS
            .iter()
            .map(|s| s.to_string())
            .collect();

        let unlisted: Vec<_> = read.difference(&listed).collect();
        let stale: Vec<_> = listed.difference(&read).collect();

        assert!(
            unlisted.is_empty(),
            "the core reads these and CORE_CONFIG_KEYS does not list them, so nothing \
             checks that a config supplies them: {unlisted:?}"
        );
        assert!(
            stale.is_empty(),
            "CORE_CONFIG_KEYS lists these and no patch reads them — either the patch \
             was dropped or the key was renamed: {stale:?}"
        );
    }

    #[test]
    fn an_optional_branch_is_all_or_nothing() {
        // The escape hatch that lets a persona without voices skip `speech`
        // must not also let a config carry a `speech` that is empty. Absent is
        // a decision; present-and-half-filled is the bug the guard exists for,
        // and one rule away from the other.
        let mut config = load("macos-15-m-series-1728x1117").derive_core_config(1, &ctx());
        assert!(
            crate::fingerprint::check_core_config(&config).is_ok(),
            "the shipped persona has no voices, so no speech branch, and that passes"
        );

        config["speech"] = serde_json::json!({});
        let missing = crate::fingerprint::check_core_config(&config)
            .expect_err("an empty speech branch must not pass");
        assert!(
            missing.iter().any(|m| m.contains("speech.voices")),
            "expected speech.voices to be named, got {missing:?}"
        );

        config["mediaDevices"] = serde_json::json!({});
        let missing = crate::fingerprint::check_core_config(&config).unwrap_err();
        assert!(
            missing.iter().any(|m| m.contains("mediaDevices.")),
            "an empty mediaDevices object reads to the core exactly like a missing \
             one, so it must fail the same way; got {missing:?}"
        );
    }

    #[test]
    fn a_persona_with_devices_reports_their_counts() {
        // The other half of the rule above: when the data is there it is used,
        // and the counts are per kind rather than a total.
        let mut persona = load("macos-15-m-series-1728x1117");
        for (kind, n) in [("audioinput", 2), ("audiooutput", 3), ("videoinput", 1)] {
            for _ in 0..n {
                persona.media_devices.push(super::PersonaMediaDevice {
                    kind: kind.to_string(),
                });
            }
        }
        let config = persona.derive_core_config(1, &ctx());
        assert_eq!(config["mediaDevices"]["audioInputCount"], 2);
        assert_eq!(config["mediaDevices"]["audioOutputCount"], 3);
        assert_eq!(config["mediaDevices"]["videoInputCount"], 1);
        crate::fingerprint::check_core_config(&config).expect("still complete");
    }

    #[test]
    fn the_validation_model_is_not_the_wire_format() {
        // These two shapes look interchangeable and are not: FingerprintConfig
        // is snake_case and predates the client-hints and webglParams sections.
        // Sending it to the core would miss every lookup, so the check that
        // guards the launch path must reject it outright.
        let sample = serde_json::to_value(crate::fingerprint::samples::macos_arm64()).unwrap();
        assert!(
            crate::fingerprint::check_core_config(&sample).is_err(),
            "FingerprintConfig must not pass as a core config — if it now does, \
             the two shapes have converged and the launcher comment is stale"
        );
    }

    #[test]
    fn webgl_vendor_is_the_chrome_constant_not_the_driver() {
        // The check a collector runs is equality against a known constant, and
        // the tell is VENDOR == UNMASKED_VENDOR_WEBGL — a pair no real browser
        // ever produces.
        let p = load("windows-11-rtx4060-1920x1080");
        let c = p.derive_core_config(7, &ctx());
        let params = &c["gpu"]["webglParams"];

        assert_eq!(params["VENDOR"], "WebKit");
        assert_eq!(params["RENDERER"], "WebKit WebGL");
        assert_ne!(
            params["VENDOR"], params["UNMASKED_VENDOR_WEBGL"],
            "VENDOR still carries the driver string"
        );
        assert!(
            params["UNMASKED_RENDERER_WEBGL"].as_str().unwrap().contains("ANGLE"),
            "the driver string was lost instead of moved"
        );
    }

    #[test]
    fn derivation_is_deterministic() {
        let p = load("windows-11-rtx4060-1920x1080");
        assert_eq!(p.derive_core_config(42, &ctx()), p.derive_core_config(42, &ctx()));
    }

    #[test]
    fn different_seeds_change_noise_but_not_the_device() {
        let p = load("windows-11-rtx4060-1920x1080");
        let a = p.derive_core_config(1, &ctx());
        let b = p.derive_core_config(2, &ctx());
        assert_ne!(a["noise"]["canvasSeed"], b["noise"]["canvasSeed"]);
        // The machine is the persona's, not the seed's — two profiles on the
        // same persona must look like two users of the same model of laptop,
        // not like two different machines.
        assert_eq!(a["navigator"]["hardwareConcurrency"], b["navigator"]["hardwareConcurrency"]);
        assert_eq!(a["screen"]["width"], b["screen"]["width"]);
        assert_eq!(a["gpu"]["webglParams"]["UNMASKED_RENDERER_WEBGL"],
                   b["gpu"]["webglParams"]["UNMASKED_RENDERER_WEBGL"]);
    }

    #[test]
    fn noise_streams_are_independent() {
        // Adding a vector later must not shift an existing one, or every aged
        // account silently changes identity on upgrade.
        assert_ne!(sub_seed(7, "canvas"), sub_seed(7, "audio"));
        assert_ne!(sub_seed(7, "canvas"), sub_seed(7, "clientRects"));
    }

    #[test]
    fn derived_config_carries_the_client_hints_chrome_brand() {
        // Phase 0 measured a clean Chromium omitting "Google Chrome" from the
        // brand list, which is how it was told apart from real Chrome.
        let p = load("macos-15-m-series-1728x1117");
        let c = p.derive_core_config(1, &ctx());
        let brands = c["clientHints"]["brands"].as_array().unwrap();
        assert!(brands.iter().any(|b| b.as_str().unwrap().starts_with("Google Chrome/")));
    }

    #[test]
    fn a_persona_contradicting_its_os_is_rejected() {
        let mut p = load("macos-15-m-series-1728x1117");
        p.chrome_metrics.scrollbar_width = 15;
        assert!(p.validate().is_err());

        let mut p = load("windows-11-rtx4060-1920x1080");
        p.gpu.webgl_renderer = "ANGLE (Apple, ANGLE Metal Renderer: Apple M3)".into();
        assert!(p.validate().is_err());
    }
}
