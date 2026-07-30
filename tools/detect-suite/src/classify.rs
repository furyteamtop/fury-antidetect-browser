//! How to read a difference between two fingerprint dumps.
//!
//! The naive tool says "these 400 fields differ" and is useless. The question is
//! always *which kind* of difference this is, and that depends on what the two
//! dumps are:
//!
//! * **Identity mode** — same browser, same machine, two runs. Almost nothing
//!   should differ. Anything that does is unstable, and instability is worse
//!   than no spoofing: a site that reads a value twice sees one identity on
//!   changing hardware.
//!
//! * **Spoof mode** — real Chrome baseline vs Fury. Plenty *should* differ —
//!   that is the whole point. But only *values* may differ. Any difference in
//!   **behaviour** (an API that exists in one and not the other, a getter that
//!   stopped being native, a codec that stopped being supported, a stability
//!   flag that flipped) means Fury is distinguishable from Chrome, which is the
//!   thing we are trying to avoid.
//!
//! So: value differences are expected in spoof mode and forbidden in identity
//! mode; behavioural differences are forbidden in both.

/// Fields whose value legitimately differs between any two runs anywhere, and
/// which therefore carry no signal for either mode.
///
/// Kept deliberately short. Every entry here is a field this tool has gone
/// blind to, so each needs a reason.
pub const NOISE: &[&str] = &[
    "__collectedInMs",                 // wall clock
    "engine.performanceNowPrecision",  // measured by sampling, jitters
    "engine.jsHeapSizeLimit",          // varies with process state
    "engine.totalJSHeapSize",
    "engine.misc.connectionRtt",       // live network measurement
    "engine.misc.connectionDownlink",
    "storage.quota.quota",             // free disk space
    "storage.quota.usage",
    "audio.offline.sum",               // float sum, last-bit noise between runs
    // Window geometry: the two dumps are taken in differently sized windows.
    // The OS-revealing derivatives (chromeHeight, scrollbarWidth) are NOT here.
    "screen.innerWidth",
    "screen.innerHeight",
    "screen.outerWidth",
    "screen.outerHeight",
    "screen.screenX",
    "screen.screenY",
    "screen.visualViewportWidth",
    "screen.visualViewportScale",
    "screen.availLeft",
    "screen.availTop",
    // Per-origin salted, so it differs by page URL rather than by browser.
    "mediaDevices.deviceIdsHash",
    "mediaDevices.groupIdsHash",
    // The mDNS candidate is a fresh random UUID on every peer connection.
    "webrtc.raw",
    "webrtc.addresses",
    "webrtc.candidateCount",
];

/// Fields that describe **behaviour**, not a value.
///
/// A difference here is a finding in both modes: it means a site can tell the
/// two browsers apart by *what they can do*, and no amount of value spoofing
/// hides that.
pub const BEHAVIOURAL: &[&str] = &[
    // "Is the spoof stable?" — must be true everywhere.
    "canvas2d.stableAcrossCalls",
    "webgl.webgl1.render.stableAcrossCalls",
    "webgl.webgl2.render.stableAcrossCalls",
    "audio.stableAcrossCalls",
    // "Are the getters native?" — PATCHED means a JS-injection spoofer.
    "engine.nativeToString",
    "engine.hasCaptureStackTrace",
    "engine.toStringTag",
    // Automation traces.
    "engine.automation.webdriver",
    "engine.automation.cdcVars",
    "engine.automation.documentCdc",
    "engine.automation.hasChromeObject",
    "engine.automation.chromeKeys",
    "engine.automation.chromeRuntime",
    "engine.automation.hasChromeLoadTimes",
    "engine.automation.permissionsQueryNative",
    // Codec and DRM capability. A build without proprietary_codecs or Widevine
    // is identified in two lines of JS regardless of its user agent.
    "codecs.hasProprietaryCodecs",
    // API presence. Absence on a claimed platform is as telling as a wrong value.
    "navigator.hasBluetooth",
    "navigator.hasUsb",
    "navigator.hasHid",
    "navigator.hasSerial",
    "navigator.hasCredentials",
    "navigator.hasGpu",
    "navigator.hasInk",
    "engine.misc.hasBattery",
    "engine.misc.hasNetworkInfo",
    "engine.misc.hasComputePressure",
    "engine.misc.hasDevicePosture",
    "fonts.hasQueryLocalFonts",
    "fonts.fontsCheckDiscriminates",
    "storage.hasLocalStorage",
    "storage.hasIndexedDB",
    // Self-consistency of the Permissions API.
    "permissions.__consistent",
    // Cross-context consistency: the single most important result.
    "crossContext.consistent",
    // Engine identity. These must not change: a different stack trace shape or
    // error message means a different engine build, not a different profile.
    "engine.errorStackShape",
    "engine.errorMessage",
    "engine.mathTan",
    "engine.mathSinh",
];

/// Prefixes that are pure identity values: expected to differ in spoof mode,
/// forbidden to differ in identity mode.
pub const VALUE_PREFIXES: &[&str] = &[
    "navigator.",
    "clientHints.",
    "screen.",
    "canvas2d.",
    "clientRects.",
    "webgl.",
    "webgpu.",
    "audio.",
    "fonts.",
    "mediaDevices.",
    "locale.",
    "css.",
    "speech.",
    "contexts.",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Carries no signal. Ignored in both modes.
    Noise,
    /// A capability or self-consistency difference. Fails in both modes.
    Behavioural,
    /// An identity value. Fails only in identity mode.
    Value,
}

pub fn classify(path: &str) -> Kind {
    if NOISE.iter().any(|p| path == *p || path.starts_with(&format!("{p}."))) {
        return Kind::Noise;
    }
    if BEHAVIOURAL.iter().any(|p| path == *p) {
        return Kind::Behavioural;
    }
    // A leaf named `__error`, `__absent` or `__timeout` anywhere means one side
    // could not read something the other could — a capability difference.
    if path.ends_with("__error") || path.ends_with("__absent") || path.ends_with("__timeout") {
        return Kind::Behavioural;
    }
    if VALUE_PREFIXES.iter().any(|p| path.starts_with(p)) {
        return Kind::Value;
    }
    // Anything unrecognised is treated as behavioural. Fail loud rather than
    // silently ignore a vector nobody classified yet.
    Kind::Behavioural
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_hash_is_a_value_but_its_stability_is_behaviour() {
        assert_eq!(classify("canvas2d.toDataURL"), Kind::Value);
        assert_eq!(classify("canvas2d.stableAcrossCalls"), Kind::Behavioural);
    }

    #[test]
    fn window_geometry_is_noise_but_chrome_height_is_not() {
        assert_eq!(classify("screen.outerHeight"), Kind::Noise);
        assert_eq!(classify("screen.chromeHeight"), Kind::Value);
        assert_eq!(classify("screen.scrollbarWidth"), Kind::Value);
    }

    #[test]
    fn unknown_paths_fail_loud() {
        assert_eq!(classify("somethingNobodyClassified"), Kind::Behavioural);
    }

    #[test]
    fn error_leaves_are_capability_differences() {
        assert_eq!(classify("drm.com.widevine.alpha.__error"), Kind::Behavioural);
        assert_eq!(classify("webgpu.__absent"), Kind::Behavioural);
    }

    #[test]
    fn noise_matches_prefix_not_substring() {
        assert_eq!(classify("webrtc.addresses"), Kind::Noise);
        assert_eq!(classify("webrtc.usesMdns"), Kind::Behavioural);
    }
}
