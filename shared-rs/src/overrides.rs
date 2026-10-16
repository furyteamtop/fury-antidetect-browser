// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Per-profile changes to a persona, field by field.
//!
//! The catalogue argues against independent dropdowns (see `catalogue.rs`),
//! and that argument still holds for the fields left out of here: the OS, the
//! platform, the UA, the fonts, the audio device. Those are what makes a
//! machine a Windows machine or a Mac, and they move only by picking another
//! persona.
//!
//! What IS here is what varies between two real computers of the same family,
//! the same axes the catalogue's own variants vary on: the screen, the cores,
//! the memory, the GPU. Operators asked to set those by hand, and a picker that
//! offers only values the OS actually reports, followed by the same
//! `Persona::validate` every catalogue entry passes, keeps them from building a
//! machine nobody ships. An override is applied to a copy of the persona, and
//! the copy is what gets derived; the core never learns anything was changed.
//!
//! Timezone and languages are not in this struct because they were per-profile
//! long before it existed and have their own columns everywhere.

use serde::{Deserialize, Serialize};

use crate::locale;
use crate::persona::Persona;

/// Everything a profile may pin about its machine. Every field `None` means
/// "as the persona has it", and an empty struct is the ordinary case.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineOverrides {
    /// The browser's own UI locale (`--lang`, and what `Intl` resolves).
    /// `None` derives it from the first language, as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<ScreenOverride>,
    /// `navigator.hardwareConcurrency`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cores: Option<u32>,
    /// `navigator.deviceMemory`, and through it the JS heap ceiling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<u32>,
    /// The id of another persona of the same OS whose GPU this one takes,
    /// whole: vendor, renderer, WebGL parameters and extensions, WebGPU.
    ///
    /// A persona id rather than a renderer string, so the only GPUs on offer
    /// are ones somebody measured or documented, with the parameters that go
    /// with them — never a typed-in renderer beside another card's limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
    /// A fixed position instead of the exit's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geolocation: Option<GeoOverride>,
    /// Canvas and WebGL readback: noised (the default) or this machine's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas: Option<CanvasMode>,
    /// NetworkInformation values for this profile's exit, overriding defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub netinfo: Option<NetInfoOverride>,
    /// Additional host-installed families to hide, beyond the persona's list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fonts_hidden: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetInfoOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downlink: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_data: Option<bool>,
}

pub fn default_netinfo() -> serde_json::Value {
    serde_json::json!({"effectiveType": "4g", "downlink": 5.5, "rtt": 150, "saveData": false})
}

/// What a page gets when it reads a canvas back.
///
/// Noise is the default and stays it. It is what keeps two profiles on one
/// machine from sharing a canvas hash -- the most direct way to link accounts
/// that run side by side -- and changing it on a profile that is already
/// trusted changes that profile's fingerprint.
///
/// Real is the machine's own rendering, byte for byte. A tester who has run
/// Sphere and Vision asked for it, and measurement backs him: pixelscan reports
/// "Masking detected" for our noise and nothing for a real Chrome (27.09.2026),
/// and no real machine has a canvas that no other machine shares. It fits one
/// account per machine, or a persona captured from this same machine, where
/// the WebGL the profile claims is the GPU that actually draws. Both 2D canvas
/// (0030) and WebGL readPixels (0031) follow it: they read the same seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CanvasMode {
    Noise,
    Real,
}

/// Marks a config whose canvas is real on purpose, so the launch check can tell
/// it from one that forgot the seed. The core never reads it.
pub const REAL_CANVAS_MARKER: &str = "canvas";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScreenOverride {
    pub width: u32,
    pub height: u32,
    pub device_pixel_ratio: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoOverride {
    pub latitude: f64,
    pub longitude: f64,
}

/// Resolutions offered per family, in logical pixels.
///
/// The Mac list is Apple's built-in panels at their default scaling plus the
/// two external monitors most often plugged into one; the Windows list is the
/// Steam survey's desktop resolutions above half a percent. Both are the
/// shapes a crowd has, which is the only reason to offer a shape at all.
const MAC_SCREENS: &[(u32, u32)] = &[
    (1280, 800),
    (1440, 900),
    (1470, 956),
    (1512, 982),
    (1680, 1050),
    (1710, 1112),
    (1728, 1117),
    (1920, 1080),
    (2560, 1440),
];
const WINDOWS_SCREENS: &[(u32, u32)] = &[
    (1280, 720),
    (1280, 1024),
    (1366, 768),
    (1440, 900),
    (1536, 864),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
    (2560, 1080),
    (2560, 1440),
    (3440, 1440),
    (3840, 2160),
];
const MAC_DPR: &[f64] = &[1.0, 2.0];
const WINDOWS_DPR: &[f64] = &[1.0, 1.25, 1.5, 1.75, 2.0];
const CORES: &[u32] = &[2, 4, 6, 8, 10, 12, 14, 16, 20, 24, 32];
const MEMORY_GB: &[u32] = &[2, 4, 8, 16, 32];

/// What a picker may offer for one persona. Filtered by its OS, so the list
/// itself cannot put an NVIDIA card on a Mac.
#[derive(Debug, Clone, Serialize)]
pub struct OverrideOptions {
    pub screens: Vec<(u32, u32)>,
    pub device_pixel_ratios: Vec<f64>,
    pub cores: Vec<u32>,
    pub memory_gb: Vec<u32>,
    pub gpus: Vec<GpuOption>,
    pub ui_locales: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuOption {
    /// The persona the GPU comes from.
    pub id: String,
    /// The unmasked renderer, which is what anybody picking a GPU recognises.
    pub renderer: String,
}

pub fn options_for(persona: &Persona, catalogue: &[Persona]) -> OverrideOptions {
    let mac = persona.os.name == "macOS";
    let mut gpus: Vec<GpuOption> = Vec::new();
    for p in catalogue.iter().filter(|p| p.os.name == persona.os.name) {
        if !gpus.iter().any(|g| g.renderer == p.gpu.webgl_renderer) {
            gpus.push(GpuOption {
                id: p.id.clone(),
                renderer: p.gpu.webgl_renderer.clone(),
            });
        }
    }
    gpus.sort_by(|a, b| a.renderer.cmp(&b.renderer));
    OverrideOptions {
        // A handset's screen and density are the model, not a setting: the
        // only honest choice offered is the one the phone has.
        screens: if persona.is_android() {
            vec![(persona.screen.width, persona.screen.height)]
        } else if mac {
            MAC_SCREENS.to_vec()
        } else {
            WINDOWS_SCREENS.to_vec()
        },
        device_pixel_ratios: if persona.is_android() {
            vec![persona.screen.device_pixel_ratio]
        } else if mac {
            MAC_DPR.to_vec()
        } else {
            WINDOWS_DPR.to_vec()
        },
        cores: CORES.to_vec(),
        // Device Memory stops at 8 on Android (persona.rs, validate_mobile).
        memory_gb: MEMORY_GB.iter().copied().filter(|&m| !persona.is_android() || m <= 8).collect(),
        gpus,
        ui_locales: locale::shipped_ui_locales()
            .into_iter()
            .map(str::to_string)
            .collect(),
    }
}

impl MachineOverrides {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The pins that live in the core config itself rather than in the
    /// persona or the context. Real canvas means no canvas
    /// seed, which is how patches 0030 and 0031 are told to leave readback
    /// alone, plus a marker saying it was meant.
    pub fn apply_config(&self, config: &mut serde_json::Value) {
        if let Some(netinfo) = &self.netinfo {
            let mut values = default_netinfo();
            if let Some(existing) = config["netinfo"].as_object() {
                values.as_object_mut().unwrap().extend(existing.clone());
            }
            let supplied = serde_json::to_value(netinfo).expect("validated netinfo");
            values.as_object_mut().unwrap().extend(supplied.as_object().unwrap().clone());
            config["netinfo"] = values;
        }
        if let Some(hidden) = &self.fonts_hidden {
            let mut names: Vec<String> = config["fontsHidden"].as_array()
                .into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
            for name in hidden {
                if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                    names.push(name.clone());
                }
            }
            config["fontsHidden"] = serde_json::json!(names);
        }
        if self.canvas == Some(CanvasMode::Real) {
            if let Some(noise) = config.get_mut("noise").and_then(|n| n.as_object_mut()) {
                noise.remove("canvasSeed");
                noise.insert(REAL_CANVAS_MARKER.into(), serde_json::json!("real"));
            }
        }
    }

    /// The two pins that live in the context rather than the persona: the UI
    /// locale and the position. Everything else in the context still comes
    /// from the exit or from the profile's own timezone and languages.
    pub fn apply_context(&self, ctx: &mut crate::persona::ProfileContext) {
        if let Some(ui) = &self.ui_locale {
            ctx.ui_locale = ui.clone();
        }
        if let Some(g) = self.geolocation {
            ctx.geolocation = Some((g.latitude, g.longitude));
        }
    }

    /// The persona as this profile presents it: a copy with every pinned field
    /// replaced, validated as a whole.
    ///
    /// `catalogue` is where a GPU donor is looked up — the caller's, because
    /// the agent knows personas from `FURY_PERSONAS` that the shared crate
    /// does not.
    ///
    /// Errors are plain sentences, the same shape `validate` produces, so the
    /// dialog shows them as they are and a launch refuses with them.
    pub fn apply(&self, base: &Persona, catalogue: &[Persona]) -> Result<Persona, Vec<String>> {
        let mut p = base.clone();
        let mut errs = Vec::new();

        if let Some(donor_id) = &self.gpu {
            match catalogue.iter().find(|d| &d.id == donor_id) {
                Some(donor) if donor.os.name != base.os.name => errs.push(format!(
                    "the GPU of {donor_id} is a {} GPU and this machine runs {}",
                    donor.os.name, base.os.name
                )),
                Some(donor) => p.gpu = donor.gpu.clone(),
                None => errs.push(format!("no machine {donor_id:?} to take a GPU from")),
            }
        }

        if let Some(s) = self.screen {
            if !(640..=7680).contains(&s.width) || !(480..=4320).contains(&s.height) {
                errs.push(format!("screen {}×{} is not a monitor", s.width, s.height));
            } else {
                // The persona's own taskbar or menu bar, carried over rather
                // than recomputed: it was measured, and a Windows machine at
                // 150% loses a different number of logical pixels than at 100%.
                let reserved_h = base.screen.height.saturating_sub(base.screen.avail_height);
                let reserved_w = base.screen.width.saturating_sub(base.screen.avail_width);
                p.screen.width = s.width;
                p.screen.height = s.height;
                p.screen.avail_width = s.width.saturating_sub(reserved_w);
                p.screen.avail_height = s.height.saturating_sub(reserved_h);
                p.screen.device_pixel_ratio = s.device_pixel_ratio;
            }
        }

        if let Some(cores) = self.cores {
            p.cpu.cores = cores;
        }
        if let Some(memory) = self.memory_gb {
            p.memory_gb = memory;
        }

        if let Some(ui) = &self.ui_locale {
            if !locale::is_shipped_ui_locale(ui) {
                errs.push(format!(
                    "{ui:?} is not a language Chrome's interface ships in"
                ));
            }
        }

        if let Some(g) = self.geolocation {
            if !(-90.0..=90.0).contains(&g.latitude) || !(-180.0..=180.0).contains(&g.longitude) {
                errs.push(format!(
                    "{}, {} is not a place on Earth",
                    g.latitude, g.longitude
                ));
            }
        }

        if let Some(n) = &self.netinfo {
            if n.effective_type.as_deref().is_some_and(|v| !["slow-2g", "2g", "3g", "4g"].contains(&v)) {
                errs.push("netinfo.effectiveType must be slow-2g, 2g, 3g or 4g".into());
            }
            if n.downlink.is_some_and(|v| !v.is_finite() || !(0.0..=10.0).contains(&v)
                || (v * 20.0 - (v * 20.0).round()).abs() > 1e-6) {
                errs.push("netinfo.downlink must be 0–10 Mbps in 0.05 Mbps steps".into());
            }
            if n.rtt.is_some_and(|v| v > 3000 || v % 50 != 0) {
                errs.push("netinfo.rtt must be 0–3000 ms in 50 ms steps".into());
            }
        }
        if self.fonts_hidden.as_ref().is_some_and(|names| names.iter().any(|n| n.trim().is_empty())) {
            errs.push("hidden font families must not be blank".into());
        }

        if let Err(more) = p.validate() {
            errs.extend(more);
        }
        if errs.is_empty() { Ok(p) } else { Err(errs) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue;

    fn persona(id: &str) -> Persona {
        catalogue::all().into_iter().find(|p| p.id == id).unwrap()
    }

    #[test]
    fn no_overrides_is_the_persona_itself() {
        let all = catalogue::all();
        for p in &all {
            let got = MachineOverrides::default().apply(p, &all).unwrap();
            assert_eq!(
                serde_json::to_value(&got).unwrap(),
                serde_json::to_value(p).unwrap(),
                "{}",
                p.id
            );
        }
    }

    #[test]
    fn every_offered_value_is_accepted_on_its_own() {
        // A picker that offers a value the validator then refuses is a trap.
        // Each option alone, on every persona, must produce a valid machine.
        let all = catalogue::all();
        for p in &all {
            let o = options_for(p, &all);
            for &(w, h) in &o.screens {
                for &dpr in &o.device_pixel_ratios {
                    let ov = MachineOverrides {
                        screen: Some(ScreenOverride { width: w, height: h, device_pixel_ratio: dpr }),
                        ..Default::default()
                    };
                    // A fractional physical pixel is a real refusal (1366 at
                    // 1.25), so only the combinations validate accepts on the
                    // unmodified screen are required to pass.
                    let physical_ok = [w, h]
                        .iter()
                        .all(|&v| ((v as f64 * dpr) - (v as f64 * dpr).round()).abs() < 1e-6);
                    assert_eq!(ov.apply(p, &all).is_ok(), physical_ok, "{} {w}×{h}@{dpr}", p.id);
                }
            }
            for &c in &o.cores {
                let ov = MachineOverrides { cores: Some(c), ..Default::default() };
                let low_memory = p.memory_gb < 4 && c > 8;
                assert_eq!(ov.apply(p, &all).is_ok(), !low_memory, "{} cores {c}", p.id);
            }
            for &m in &o.memory_gb {
                let ov = MachineOverrides { memory_gb: Some(m), ..Default::default() };
                let low_memory = m < 4 && p.cpu.cores > 8;
                assert_eq!(ov.apply(p, &all).is_ok(), !low_memory, "{} memory {m}", p.id);
            }
            for g in &o.gpus {
                let ov = MachineOverrides { gpu: Some(g.id.clone()), ..Default::default() };
                ov.apply(p, &all)
                    .unwrap_or_else(|e| panic!("{} with the GPU of {}: {e:?}", p.id, g.id));
            }
            for ui in &o.ui_locales {
                let ov = MachineOverrides { ui_locale: Some(ui.clone()), ..Default::default() };
                assert!(ov.apply(p, &all).is_ok(), "{ui}");
            }
        }
    }

    #[test]
    fn a_gpu_never_crosses_families() {
        let all = catalogue::all();
        let mac = persona("macos-m1-1440x900");
        let options = options_for(&mac, &all);
        assert!(options.gpus.iter().all(|g| !g.renderer.contains("Direct3D")));
        let ov = MachineOverrides {
            gpu: Some("win11-rtx4060-1920x1080".into()),
            ..Default::default()
        };
        assert!(ov.apply(&mac, &all).is_err());
    }

    #[test]
    fn a_screen_keeps_the_personas_taskbar() {
        let all = catalogue::all();
        let win = persona("win11-rtx4060-1920x1080");
        let ov = MachineOverrides {
            screen: Some(ScreenOverride { width: 2560, height: 1440, device_pixel_ratio: 1.0 }),
            ..Default::default()
        };
        let p = ov.apply(&win, &all).unwrap();
        assert_eq!(p.screen.height - p.screen.avail_height, win.screen.height - win.screen.avail_height);
        assert_eq!(p.screen.width, 2560);
    }

    #[test]
    fn an_impossible_machine_is_refused() {
        let all = catalogue::all();
        let win = persona("win11-rtx4060-1920x1080");
        for ov in [
            MachineOverrides { cores: Some(7), ..Default::default() },
            MachineOverrides { memory_gb: Some(12), ..Default::default() },
            MachineOverrides { ui_locale: Some("xx".into()), ..Default::default() },
            MachineOverrides {
                geolocation: Some(GeoOverride { latitude: 91.0, longitude: 0.0 }),
                ..Default::default()
            },
            MachineOverrides {
                screen: Some(ScreenOverride { width: 1366, height: 768, device_pixel_ratio: 1.25 }),
                ..Default::default()
            },
        ] {
            assert!(ov.apply(&win, &all).is_err(), "{ov:?}");
        }
    }

    #[test]
    fn real_canvas_drops_the_seed_and_says_so_and_noise_changes_nothing() {
        let config = || serde_json::json!({ "noise": { "canvasSeed": 7, "audioSeed": 9 } });

        let mut real = config();
        MachineOverrides { canvas: Some(CanvasMode::Real), ..Default::default() }.apply_config(&mut real);
        assert!(real["noise"].get("canvasSeed").is_none());
        assert_eq!(real["noise"]["canvas"], "real");
        assert_eq!(real["noise"]["audioSeed"], 9, "audio noise is not canvas");

        for ov in [MachineOverrides::default(), MachineOverrides { canvas: Some(CanvasMode::Noise), ..Default::default() }] {
            let mut same = config();
            ov.apply_config(&mut same);
            assert_eq!(same, config());
        }
        assert_eq!(
            serde_json::to_string(&MachineOverrides { canvas: Some(CanvasMode::Real), ..Default::default() }).unwrap(),
            r#"{"canvas":"real"}"#
        );
    }

    #[test]
    fn an_empty_struct_serialises_to_an_empty_object() {
        // What every existing profile stores, and what the server's column
        // defaults to.
        assert_eq!(serde_json::to_string(&MachineOverrides::default()).unwrap(), "{}");
        let back: MachineOverrides = serde_json::from_str("{}").unwrap();
        assert!(back.is_empty());
    }

    #[test]
    fn exit_and_hidden_fonts_survive_the_profile_wire_and_reach_the_core() {
        let wire = serde_json::json!({
            "netinfo": {"effectiveType": "4g", "downlink": 5.5, "rtt": 150, "saveData": false},
            "fonts_hidden": ["CustomDesign Font", "arial"]
        });
        let ov: MachineOverrides = serde_json::from_value(wire.clone()).unwrap();
        let all = catalogue::all();
        let p = &all[0];
        ov.apply(p, &all).unwrap();
        assert_eq!(serde_json::to_value(&ov).unwrap(), wire);
        let mut config = serde_json::json!({"fontsHidden": ["Arial", "Papyrus"]});
        ov.apply_config(&mut config);
        assert_eq!(config["netinfo"], wire["netinfo"]);
        assert_eq!(config["fontsHidden"], serde_json::json!(["Arial", "Papyrus", "CustomDesign Font"]));
    }

    #[test]
    fn network_overrides_keep_defaults_and_preserve_zero_and_false() {
        let ov: MachineOverrides = serde_json::from_value(serde_json::json!({
            "netinfo": {"downlink": 0, "rtt": 0, "saveData": false}
        })).unwrap();
        let all = catalogue::all();
        ov.apply(&all[0], &all).unwrap();
        let mut config = serde_json::json!({});
        ov.apply_config(&mut config);
        assert_eq!(config["netinfo"]["rtt"], 0);
        assert_eq!(config["netinfo"]["downlink"], 0.0);
        assert_eq!(config["netinfo"]["saveData"], false);
        assert_eq!(config["netinfo"]["effectiveType"], "4g");
        for bad in [serde_json::json!({"rtt": 51}), serde_json::json!({"rtt": 3050}),
                    serde_json::json!({"downlink": -1}), serde_json::json!({"downlink": 5.51}),
                    serde_json::json!({"effectiveType": "5g"})] {
            let ov: MachineOverrides = serde_json::from_value(serde_json::json!({"netinfo": bad})).unwrap();
            assert!(ov.apply(&all[0], &all).is_err());
        }
    }
}
