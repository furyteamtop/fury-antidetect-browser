// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

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

/// Sample rates a real output device reports through `AudioContext`.
///
/// This used to be `[44100, 48000]`, and the first persona anyone sent
/// through the issue form (#5, a Windows 10 desktop with a GTX 950) was
/// refused for reporting 192000 — the rate its owner had picked in the Sound
/// control panel, where Windows offers every value below. The check exists
/// to catch a number nobody's hardware produces, not a real machine with an
/// unusual setting: an unusual real value is a small crowd, which is a fact
/// for the catalogue to state, not for the validator to refuse.
pub const AUDIO_SAMPLE_RATES: &[u32] = &[
    8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000,
];

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
    /// Who sent the machine, as a GitHub handle, and what they said it was.
    /// Written by the issue workflow (.github/workflows/persona-issue.yml);
    /// the two built-in personas have neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
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
    /// The handset, on an Android persona and nowhere else (docs/18).
    ///
    /// A separate block rather than more fields on `os`, because everything in
    /// it is about the device in the hand rather than the operating system: a
    /// phone and a tablet run the same Android and announce themselves
    /// differently, and the model is what Sec-CH-UA-Model carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mobile: Option<PersonaMobile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaMobile {
    /// Sec-CH-UA-Model and `getHighEntropyValues().model`: "SM-A546B".
    pub model: String,
    /// "phone" | "tablet". Chrome on an Android tablet sends the desktop-shaped
    /// user agent without "Mobile" and Sec-CH-UA-Mobile: ?0, so the two are
    /// not a detail of one another.
    pub form_factor: String,
    /// `navigator.connection.type`: "cellular" | "wifi". Desktop Chrome does
    /// not expose the attribute at all; Android always does.
    pub connection: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaOs {
    /// "Windows" | "macOS" | "Android"
    pub name: String,
    pub version: String,
    /// "x86_64" | "arm64"
    pub arch: String,
    /// The UA string template, with {CHROME_MAJOR} substituted at derive time
    /// so a persona survives a Chromium uprev without being rewritten.
    pub user_agent_template: String,
    /// navigator.platform: "Win32", "MacIntel", or "Linux armv81" and its
    /// siblings on Android (see `ANDROID_PLATFORMS`).
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
    /// Where the exit says it is, as (latitude, longitude), or `None` when the
    /// exit could not be resolved.
    ///
    /// `None` is not a failure to paper over. Patch 0082 falls through to the
    /// platform's own provider when no position is configured, so a profile
    /// whose exit is unknown answers like the machine it is running on rather
    /// than like a machine somewhere invented. Answering with a guess is how a
    /// profile ends up claiming Berlin while its clock says New York.
    pub geolocation: Option<(f64, f64)>,
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
    // Masked to 31 bits because the core reads these through
    // FuryConfig::GetInt, which is base::Value's int — 32-bit and SIGNED. A
    // seed above 2^31 does not clamp or error there; GetIfInt returns nullopt,
    // GetCanvasNoiseSeed answers false, and the surface is left entirely
    // unnoised while every log line still says the profile is configured.
    // verify-0033.py demonstrated it by accident with a seed of 0xC1EC7001:
    // identical geometry across two seeds and the unconfigured build.
    (mix(h) & 0x7fff_ffff) as u32
}

/// The brand list real Chrome of this major version sends, in its order.
///
/// Chrome does not have one GREASE brand: it derives it from the major version
/// (components/embedder_support/user_agent_utils.cc, GenerateBrandVersionList),
/// and the order of the three entries too. This used to be the literal list of
/// Chrome 150 — "Not;A=Brand/8, Chromium, Google Chrome" — and stayed that way
/// through the move to 153, whose real Chrome sends "Google Chrome, Not_A
/// Brand/8, Chromium" (baselines/chrome-153-windows-x64.json, 27.09.2026). Every
/// profile carried a Sec-CH-UA no Chrome 153 sends. Ported from the 155 source
/// so the next milestone moves it by itself.
pub fn chrome_brand_list(major: u32, version: &str, full: bool) -> Vec<String> {
    const GREASEY_CHARS: [&str; 11] = [" ", "(", ":", "-", ".", "/", ")", ";", "=", "?", "_"];
    const GREASED_VERSIONS: [&str; 3] = ["8", "99", "24"];
    const ORDERS: [[usize; 3]; 6] =
        [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let seed = major as usize;
    let grease_brand = format!(
        "Not{}A{}Brand",
        GREASEY_CHARS[seed % GREASEY_CHARS.len()],
        GREASEY_CHARS[(seed + 1) % GREASEY_CHARS.len()]
    );
    let grease_version = GREASED_VERSIONS[seed % GREASED_VERSIONS.len()];
    let grease_version = if full { format!("{grease_version}.0.0.0") } else { grease_version.to_string() };
    let listed = [
        format!("{grease_brand}/{grease_version}"),
        format!("Chromium/{version}"),
        format!("Google Chrome/{version}"),
    ];
    // ShuffleBrandList: entry i goes to position order[i].
    let order = ORDERS[seed % ORDERS.len()];
    let mut out = vec![String::new(); 3];
    for (i, entry) in listed.into_iter().enumerate() {
        out[order[i]] = entry;
    }
    out
}

/// What `navigator.platform` says on Android. "Linux armv81" is what current
/// phones are seen answering, "Linux armv8l" and "Linux armv7l" older or
/// 32-bit builds, "Linux aarch64" a few devices. Not yet checked against our
/// own captures (docs/18, step 6); the list only refuses what no phone says.
pub const ANDROID_PLATFORMS: &[&str] =
    &["Linux armv81", "Linux armv8l", "Linux armv7l", "Linux aarch64"];

/// Driver families that ship in Android handsets. A renderer naming none of
/// them is a desktop GPU wearing a phone.
pub const ANDROID_GPU_FAMILIES: &[&str] =
    &["Adreno", "Mali", "PowerVR", "Xclipse", "Immortalis", "Maleoon", "IMG"];

impl Persona {
    pub fn is_android(&self) -> bool {
        self.os.name == "Android"
    }

    /// Sec-CH-UA-Mobile: ?1. A phone; not a tablet, whose Chrome asks for the
    /// desktop site and says so.
    pub fn is_phone(&self) -> bool {
        self.is_android() && self.mobile.as_ref().is_some_and(|m| m.form_factor == "phone")
    }

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

        let brands = chrome_brand_list(ctx.chrome_major, &ctx.chrome_major.to_string(), false);
        let full_version_list =
            chrome_brand_list(ctx.chrome_major, &ctx.chrome_full_version, true);

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
                "model": self.mobile.as_ref().map(|m| m.model.as_str()).unwrap_or(""),
                "mobile": self.is_phone(),
                "wow64": false,
                "fullVersion": ctx.chrome_full_version,
                // Empty is what a desktop Chrome sends. The header exists and
                // is read; omitting the key left the patch falling back to
                // whatever the host reports, which on a phone-shaped host
                // would contradict Sec-CH-UA-Mobile: ?0.
                //
                // A phone and a tablet name themselves, as Chrome on Android
                // does (user_agent_utils.cc, GetFormFactorsClientHint).
                "formFactors": match self.mobile.as_ref().map(|m| m.form_factor.as_str()) {
                    Some("phone") => vec!["Mobile"],
                    Some("tablet") => vec!["Tablet"],
                    _ => Vec::<&str>::new(),
                },
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
            // What the core hides; wins over "fonts" in a core that knows it.
            // See fonts.rs for why hiding, not allowing.
            "fontsHidden": crate::fonts::hidden(&self.os.name, &self.fonts),
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
        //
        // A starting state, not a standing answer. Patch 0090 substitutes these
        // only while the real permission is still undecided, so a user who
        // clicks Allow gets "granted" back from permissions.query afterwards —
        // which is both true and, since patch 0082 made geolocation actually
        // answer, the only self-consistent thing to report.
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
            "jsHeapSizeLimit": if self.is_android() {
                js_heap_limit_android(self.memory_gb)
            } else {
                js_heap_limit(self.memory_gb)
            },
        });

        // Only when there is one. An absent key is what tells the core to leave
        // the platform provider alone — see FuryLocationProvider::MaybeCreate.
        if let Some((lat, lng)) = ctx.geolocation {
            config["geolocation"] = serde_json::json!({
                "latitude": lat,
                "longitude": lng,
                // Metres, and deliberately coarse. The exit checker knows a
                // city, not a doorway, and a desktop with no GPS derives its
                // position from the network — an accuracy of five metres from a
                // machine that has never seen a satellite is a claim about
                // hardware it does not have.
                "accuracy": 20_000.0,
            });
        }

        // The traces an automated Chrome leaves that a driven-but-hidden one
        // should not. On unconditionally: a profile that wants to be driven
        // says so with `cdp`, and that is a different decision.
        config["automation"] = serde_json::json!({ "hideTraces": true });

        // Not a persona property — a consequence of how Fury reaches the
        // network, and true for every profile because every profile goes
        // through the relay.
        //
        // The relay speaks HTTP and SOCKS5 CONNECT, both of which are TCP.
        // WebRTC gathers its candidates on its own UDP sockets, so a peer
        // connection reaches the network past the relay entirely and reports
        // the machine's real address. Measured on 02.08.2026: with a proxy
        // configured, a page opening an RTCPeerConnection received an srflx
        // candidate carrying this machine's real public IP. Every profile on the
        // machine would have received the same one.
        //
        // "disable_non_proxied_udp" is one of the four values Chrome's own
        // enterprise policy accepts, so a browser in this state is a shape that
        // exists in the world rather than one we invented. It becomes "default"
        // the day the relay carries UDP — one string, in one place, which is why
        // it is a config key and not a constant in the core.
        config["webrtc"] = serde_json::json!({
            "ipHandlingPolicy": "disable_non_proxied_udp",
        });

        // A desktop on mains, which is what the browser will report to every
        // page and to every profile alike.
        //
        // Not a persona property either, and deliberately not one. The real
        // battery is a machine-wide, time-varying value: every profile on one
        // host reads the same level and the same dischargingTime at the same
        // instant, so it links them to each other no matter how different their
        // proxies and canvas seeds are. Giving each profile its own invented
        // battery would trade one problem for a worse one — a level that never
        // moves while the page watches, or two profiles whose batteries drift
        // apart at impossible rates.
        //
        // So: the tuple carrying no entropy at all, which the largest
        // population of real machines reports. It is also BatteryStatus's own
        // default (battery_status.h:23-24) and what every desktop on mains
        // answers with, and a laptop left plugged in answers with it too — so
        // it contradicts nothing in the catalogue, MacBooks included.
        //
        // -1 means Infinity. JSON has no Infinity, and absent already means
        // "use the machine's real value" for every key the core reads, so
        // absent cannot be made to mean it here.
        config["battery"] = serde_json::json!({
            "charging": true,
            "level": 1.0,
            "chargingTime": 0.0,
            "dischargingTime": -1.0,
        });

        // The switch the mobile patch reads (0130): layout viewport and
        // <meta viewport>, coarse pointer without hover, touch events, the
        // screen orientation, connection.type, and the desktop-only surfaces
        // (PDF viewer, HID, Serial) taken away. Absent on a desktop persona,
        // which is what leaves every one of those to stock Chromium there.
        if let Some(m) = &self.mobile {
            config["mobile"] = serde_json::json!({
                "enabled": true,
                "formFactor": m.form_factor,
                "connectionType": m.connection,
                // The finger's habits (patch 0132): where it lands relative
                // to its target, how big and how hard. Per profile and stable,
                // like the rest of the fingerprint; its own stream, so it
                // shifts nothing else.
                "touchSeed": sub_seed(seed, "touch"),
            });
        }

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
        let is_android = self.is_android();

        if !is_mac && !is_win && !is_android {
            errs.push(format!("unknown os.name {:?}", self.os.name));
        }

        if is_android {
            if !ANDROID_PLATFORMS.contains(&self.os.platform.as_str()) {
                errs.push(format!(
                    "os.platform {:?} is not one Android reports ({ANDROID_PLATFORMS:?})",
                    self.os.platform
                ));
            }
        } else {
            let expected_platform = if is_mac { "MacIntel" } else { "Win32" };
            if self.os.platform != expected_platform {
                errs.push(format!(
                    "os.platform {:?} does not match os.name {:?} (expected {expected_platform:?})",
                    self.os.platform, self.os.name
                ));
            }
        }

        self.validate_mobile(&mut errs);

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
        // ANGLE's D3D11 backend always writes the PCI device id after the
        // adapter's name: "NVIDIA GeForce RTX 4060 (0x00002882) Direct3D11"
        // (Renderer11.cpp, getRendererDescription; and every Windows capture
        // in baselines/). Without it the string is one Chrome never shows (#16).
        if is_win && r.contains("Direct3D11") && !has_pci_device_id(r) {
            errs.push(format!(
                "Windows D3D11 renderer without the PCI device id ANGLE always writes, \
                 \"(0x0000XXXX)\" after the adapter name: {r}"
            ));
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
        // devicePixelRatio is not a free number. macOS ships exactly two —
        // Retina and not — and Windows exposes its scaling slider, which moves
        // in 25% steps. A ratio outside these is a machine that does not exist,
        // and it is one `window.devicePixelRatio` hands to any page that asks.
        let dpr = self.screen.device_pixel_ratio;
        //
        // Android has no such ladder: the ratio is the panel's density over
        // 160 dpi, rounded by the vendor (2.625, 2.75, 2.8125, 3.5 are all
        // real), so only the range is checked there and the whole-pixel rule
        // below does the rest.
        let allowed: &[f64] = if is_mac {
            &[1.0, 2.0]
        } else {
            &[1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0]
        };
        if is_android {
            if !(1.0..=4.5).contains(&dpr) {
                errs.push(format!("devicePixelRatio {dpr} is outside what Android panels report (1 to 4.5)"));
            }
        } else if !allowed.iter().any(|a| (a - dpr).abs() < 1e-9) {
            errs.push(format!(
                "devicePixelRatio {dpr} is not one {} reports ({allowed:?})",
                self.os.name
            ));
        }

        // The logical size times the ratio is the panel, and a panel cannot
        // have a fractional pixel. This catches a persona assembled by taking
        // one machine's resolution and another's scaling.
        for (what, logical) in [("width", self.screen.width), ("height", self.screen.height)] {
            let physical = logical as f64 * dpr;
            if (physical - physical.round()).abs() > 1e-6 {
                errs.push(format!(
                    "screen.{what} {logical} at devicePixelRatio {dpr} is \
                     {physical} physical pixels, which no panel has"
                ));
            }
        }

        // Logical cores. Every shipping Mac and every consumer x86 part is an
        // even number — hyperthreading and Apple's core clusters both come in
        // pairs — and `navigator.hardwareConcurrency` is read by every
        // fingerprinting script there is.
        if self.cpu.cores == 0 || self.cpu.cores % 2 != 0 || self.cpu.cores > 128 {
            errs.push(format!(
                "cpu.cores {} is not a count a real machine reports (even, 2 to 128)",
                self.cpu.cores
            ));
        }

        if self.memory_gb < 4 && self.cpu.cores > 8 {
            errs.push(format!(
                "{} GB with {} cores is not a machine that exists",
                self.memory_gb, self.cpu.cores
            ));
        }

        if !AUDIO_SAMPLE_RATES.contains(&self.audio.sample_rate) {
            errs.push(format!(
                "sample rate {} is not one a sound device offers; real machines \
                 report 44100 or 48000, a studio interface up to 192000",
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

    /// The handset rules: everything an Android persona must say about itself
    /// and a desktop one must not.
    fn validate_mobile(&self, errs: &mut Vec<String>) {
        let Some(m) = &self.mobile else {
            if self.is_android() {
                errs.push("an Android persona needs its `mobile` block (model, form_factor, connection)".into());
            }
            return;
        };
        if !self.is_android() {
            errs.push(format!("a {} persona with a `mobile` block", self.os.name));
            return;
        }

        if self.os.ch_platform != "Android" {
            errs.push(format!("os.ch_platform {:?} on an Android persona", self.os.ch_platform));
        }
        if m.model.trim().is_empty() {
            errs.push("mobile.model is empty; every Android Chrome sends Sec-CH-UA-Model".into());
        }
        let phone = match m.form_factor.as_str() {
            "phone" => true,
            "tablet" => false,
            other => {
                errs.push(format!("mobile.form_factor {other:?} is neither \"phone\" nor \"tablet\""));
                true
            }
        };
        if !matches!(m.connection.as_str(), "cellular" | "wifi") {
            errs.push(format!("mobile.connection {:?} is neither \"cellular\" nor \"wifi\"", m.connection));
        }

        // The user agent and the hints have to tell one story: a phone says
        // "Mobile" in both, a tablet in neither.
        let ua = &self.os.user_agent_template;
        if !ua.contains("Android") {
            errs.push("the user agent of an Android persona does not say Android".into());
        }
        if phone != ua.contains(" Mobile ") {
            errs.push(if phone {
                "a phone's user agent must carry \"Mobile\"".into()
            } else {
                "a tablet's user agent must not carry \"Mobile\"".into()
            });
        }

        if self.chrome_metrics.scrollbar_width != 0 {
            errs.push("Android scrollbars overlay the page; scrollbar_width must be 0".into());
        }
        if !(1..=10).contains(&self.max_touch_points) {
            errs.push(format!(
                "max_touch_points {} on a touchscreen device (phones report 5 or 10)",
                self.max_touch_points
            ));
        }
        if self.screen.color_depth != 24 {
            errs.push(format!("color_depth {} on Android, which reports 24", self.screen.color_depth));
        }
        // Device Memory caps at 8 on Android; all 50 of ShardBrowser's
        // Android personas sit at 2, 4 or 8.
        if self.memory_gb > 8 {
            errs.push(format!("memory_gb {} above the 8 Android reports", self.memory_gb));
        }

        let r = &self.gpu.webgl_renderer;
        if r.contains("Direct3D") || r.contains("D3D11") || r.contains("Metal") {
            errs.push(format!("Android persona with a desktop driver stack: {r}"));
        }
        if !ANDROID_GPU_FAMILIES.iter().any(|f| r.contains(f)) {
            errs.push(format!(
                "renderer {r:?} names no handset GPU family ({ANDROID_GPU_FAMILIES:?})"
            ));
        }

        const DESKTOP_ONLY_FONTS: &[&str] =
            &["Segoe UI", "Bahnschrift", "Calibri", "Helvetica Neue", "Menlo", "SF Pro", "Apple Color Emoji"];
        if let Some(f) = self.fonts.iter().find(|f| DESKTOP_ONLY_FONTS.contains(&f.as_str())) {
            errs.push(format!("desktop font {f:?} on an Android persona"));
        }
    }
}

/// " (0x" + eight hex digits + ")", as ANGLE formats a DXGI DeviceId.
fn has_pci_device_id(renderer: &str) -> bool {
    renderer.match_indices("(0x").any(|(i, _)| {
        let hex = &renderer[i + 3..];
        hex.len() > 8 && hex[..8].chars().all(|c| c.is_ascii_hexdigit()) && hex[8..].starts_with(')')
    })
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

/// The same ceiling on Android, where it is a plain power of two per memory
/// tier. Taken from ShardBrowser's 50 Android personas (docs/18), all of which
/// pair 2, 4 and 8 GB with 1, 2 and 4 GiB; to be confirmed by our own phone
/// captures. The desktop numbers would be a tell here: 4294705152 is what a
/// desktop V8 reports, and no phone in that set does.
fn js_heap_limit_android(memory_gb: u32) -> u64 {
    match memory_gb {
        0..=2 => 1_073_741_824,
        3..=4 => 2_147_483_648,
        _ => 4_294_967_296,
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
            geolocation: Some((40.7128, -74.0060)),
            chrome_major: 155,
            chrome_full_version: "155.0.8059.39".into(),
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

    /// A phone for the tests, NOT a catalogue entry: the shape of a Galaxy A54
    /// from its public specification, on the Windows base's WebGL table, until
    /// a phone is captured (docs/18, step 6). Only the fields the rules read
    /// are made to look like a phone.
    fn android() -> Persona {
        let mut p = load("windows-11-rtx4060-1920x1080");
        p.id = "android-test-a54".into();
        p.weight = 0.0;
        p.os.name = "Android".into();
        p.os.version = "15".into();
        p.os.arch = "arm64".into();
        p.os.platform = "Linux armv81".into();
        p.os.ch_platform = "Android".into();
        p.os.ch_platform_version = "15.0.0".into();
        p.os.ch_architecture = String::new();
        p.os.ch_bitness = String::new();
        p.os.user_agent_template = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 \
            (KHTML, like Gecko) Chrome/{CHROME_MAJOR}.0.0.0 Mobile Safari/537.36"
            .into();
        p.gpu.webgl_vendor = "Google Inc. (ARM)".into();
        p.gpu.webgl_renderer = "ANGLE (ARM, Mali-G68, OpenGL ES 3.2)".into();
        p.gpu.webgpu = None;
        p.screen = PersonaScreen {
            width: 384,
            height: 832,
            avail_width: 384,
            avail_height: 832,
            color_depth: 24,
            device_pixel_ratio: 2.8125,
        };
        p.chrome_metrics.scrollbar_width = 0;
        p.cpu.cores = 8;
        p.memory_gb = 8;
        p.max_touch_points = 5;
        p.fonts = vec!["Roboto".into(), "Noto Color Emoji".into(), "Droid Sans Mono".into()];
        p.mobile = Some(PersonaMobile {
            model: "SM-A546B".into(),
            form_factor: "phone".into(),
            connection: "cellular".into(),
        });
        p
    }

    #[test]
    fn a_windows_renderer_without_the_pci_id_is_refused() {
        let mut p = load("windows-11-rtx4060-1920x1080");
        p.validate().unwrap_or_else(|e| panic!("{e:?}"));
        p.gpu.webgl_renderer =
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 vs_5_0 ps_5_0, D3D11)".into();
        assert!(p.validate().is_err(), "#16: no device id was accepted");
        for r in crate::catalogue::all().iter().filter(|p| p.os.name == "Windows") {
            assert!(has_pci_device_id(&r.gpu.webgl_renderer), "{}: {}", r.id, r.gpu.webgl_renderer);
        }
    }

    #[test]
    fn a_phone_is_consistent_and_derives_a_phone() {
        let p = android();
        p.validate().unwrap_or_else(|e| panic!("{e:?}"));
        let c = p.derive_core_config(1, &ctx());
        assert_eq!(c["clientHints"]["mobile"], true);
        assert_eq!(c["clientHints"]["model"], "SM-A546B");
        assert_eq!(c["clientHints"]["formFactors"], serde_json::json!(["Mobile"]));
        assert_eq!(c["navigator"]["platform"], "Linux armv81");
        assert_eq!(c["screen"]["availTop"], 0);
        assert_eq!(c["mobile"]["enabled"], true);
        assert_eq!(c["mobile"]["connectionType"], "cellular");
        assert!(c["mobile"]["touchSeed"].as_u64().is_some());
        assert_ne!(
            p.derive_core_config(1, &ctx())["mobile"]["touchSeed"],
            p.derive_core_config(2, &ctx())["mobile"]["touchSeed"],
            "each profile has its own finger"
        );
        assert_eq!(c["engine"]["jsHeapSizeLimit"], 4_294_967_296u64);
        let hidden: Vec<&str> = c["fontsHidden"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        for f in ["Arial", "Segoe UI", "Helvetica"] {
            assert!(hidden.contains(&f), "{f} not hidden on a phone");
        }
        crate::fingerprint::check_core_config(&c)
            .unwrap_or_else(|missing| panic!("{}", missing.join("\n")));
    }

    #[test]
    fn a_desktop_derives_no_mobile_branch_and_says_so_in_the_hints() {
        let c = load("windows-11-rtx4060-1920x1080").derive_core_config(1, &ctx());
        assert!(c.get("mobile").is_none());
        assert_eq!(c["clientHints"]["mobile"], false);
        assert_eq!(c["clientHints"]["model"], "");
        assert_eq!(c["clientHints"]["formFactors"], serde_json::json!([]));
    }

    #[test]
    fn a_tablet_says_neither_mobile_nor_phone() {
        let mut p = android();
        p.mobile.as_mut().unwrap().form_factor = "tablet".into();
        p.os.user_agent_template = p.os.user_agent_template.replace(" Mobile ", " ");
        p.validate().unwrap_or_else(|e| panic!("{e:?}"));
        let c = p.derive_core_config(1, &ctx());
        assert_eq!(c["clientHints"]["mobile"], false);
        assert_eq!(c["clientHints"]["formFactors"], serde_json::json!(["Tablet"]));
    }

    #[test]
    fn a_phone_that_contradicts_itself_is_refused() {
        let cases: Vec<(&str, Box<dyn Fn(&mut Persona)>)> = vec![
            ("desktop platform", Box::new(|p| p.os.platform = "Win32".into())),
            ("no mobile block", Box::new(|p| p.mobile = None)),
            ("phone UA without Mobile", Box::new(|p| p.os.user_agent_template = p.os.user_agent_template.replace(" Mobile ", " "))),
            ("Metal renderer", Box::new(|p| p.gpu.webgl_renderer = "ANGLE (Apple, ANGLE Metal Renderer: Apple M5, Unspecified Version)".into())),
            ("desktop GPU", Box::new(|p| p.gpu.webgl_renderer = "ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 vs_5_0 ps_5_0, D3D11)".into())),
            ("scrollbar", Box::new(|p| p.chrome_metrics.scrollbar_width = 15)),
            ("no touch", Box::new(|p| p.max_touch_points = 0)),
            ("deep colour", Box::new(|p| p.screen.color_depth = 30)),
            ("16 GB", Box::new(|p| p.memory_gb = 16)),
            ("desktop font", Box::new(|p| p.fonts.push("Segoe UI".into()))),
            ("empty model", Box::new(|p| p.mobile.as_mut().unwrap().model.clear())),
            ("fractional panel", Box::new(|p| p.screen.device_pixel_ratio = 2.7)),
        ];
        for (what, break_it) in cases {
            let mut p = android();
            break_it(&mut p);
            assert!(p.validate().is_err(), "{what} was accepted");
        }
        let mut win = load("windows-11-rtx4060-1920x1080");
        win.mobile = android().mobile;
        assert!(win.validate().is_err(), "a Windows persona with a handset block was accepted");
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
    fn brands_are_what_real_chrome_of_that_version_sends() {
        // Measured: baselines/chrome-150-macos-arm64.json and
        // chrome-153-windows-x64.json, real Chrome, same probe.
        assert_eq!(
            chrome_brand_list(150, "150", false),
            ["Not;A=Brand/8", "Chromium/150", "Google Chrome/150"]
        );
        assert_eq!(
            chrome_brand_list(153, "153.0.8010.37", true),
            ["Google Chrome/153.0.8010.37", "Not_A Brand/8.0.0.0", "Chromium/153.0.8010.37"]
        );
        // What the 155 source computes (seed 155: "(" ":", 24, order {2,1,0}).
        assert_eq!(
            chrome_brand_list(155, "155", false),
            ["Google Chrome/155", "Chromium/155", "Not(A:Brand/24"]
        );
    }

    /// The three checks added when docs/16 asked for a persona that refuses to
    /// be impossible. Each is a value a page reads directly.
    #[test]
    fn a_machine_that_does_not_exist_is_rejected() {
        // devicePixelRatio. macOS ships Retina and not; there is no 1.5 Mac.
        let mut p = load("macos-15-m-series-1728x1117");
        p.screen.device_pixel_ratio = 1.5;
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("devicePixelRatio")), "{errs:?}");

        // Windows moves in 25% steps, so 1.25 is fine and 1.33 is not.
        let mut p = load("windows-11-rtx4060-1920x1080");
        p.screen.device_pixel_ratio = 1.25;
        assert!(p.validate().is_ok(), "{:?}", p.validate());
        p.screen.device_pixel_ratio = 1.33;
        assert!(p.validate().is_err());

        // A fractional panel. This is what taking one machine's resolution and
        // another's scaling produces.
        let mut p = load("windows-11-rtx4060-1920x1080");
        p.screen.width = 1921;
        p.screen.avail_width = 1921;
        p.screen.device_pixel_ratio = 1.5;
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("physical pixels")), "{errs:?}");

        // Logical cores. Hyperthreading and Apple's clusters both come in
        // pairs, so an odd count is a machine nobody sells.
        let mut p = load("macos-15-m-series-1728x1117");
        p.cpu.cores = 7;
        let errs = p.validate().unwrap_err();
        assert!(errs.iter().any(|e| e.contains("cpu.cores")), "{errs:?}");
        p.cpu.cores = 0;
        assert!(p.validate().is_err());
        p.cpu.cores = 256;
        assert!(p.validate().is_err());
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
