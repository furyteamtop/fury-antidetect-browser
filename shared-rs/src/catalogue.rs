// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The persona catalogue.
//!
//! # Why this is a table of machines and not a set of dropdowns
//!
//! Every commercial anti-detect browser offers independent pickers: choose a
//! user agent here, a WebGL vendor there, a renderer from a list of forty. That
//! looks like more freedom and is in fact the opposite of what it needs to be.
//! The combinations are not independent in reality — an NVIDIA renderer does
//! not appear on macOS, a 15-pixel scrollbar does not appear on a Mac, a
//! Retina device pixel ratio does not appear beside a 1366×768 screen — so the
//! product of all those lists is overwhelmingly made of devices that do not
//! exist. Picking one of those is worse than not spoofing at all: an
//! un-spoofed browser looks like a normal machine, while an impossible one is
//! a machine nobody has ever shipped.
//!
//! So variety comes from more *real* machines, not more combinations. Each
//! entry below varies only what genuinely varies between real computers of the
//! same family: the GPU model, the screen, cores and memory, the OS point
//! release. Everything else — the WebGL limits, the extension list, the font
//! set, the audio latencies — is a property of the operating system and its
//! ANGLE backend rather than of the card, so it is inherited from the measured
//! base and left alone.
//!
//! Two entries are marked `measured`: someone dumped them off a physical
//! machine. The rest are that measurement with a documented model substituted,
//! and they say so — the interface shows the difference, because "measured"
//! is a stronger claim and should not be diluted.

use crate::persona::Persona;

const MACOS_BASE: &str = include_str!("../../shared/personas/macos-15-m-series-1728x1117.json");
const WINDOWS_BASE: &str = include_str!("../../shared/personas/windows-11-rtx4060-1920x1080.json");

/// One real machine, as a delta from its family's measured base.
struct Variant {
    id: &'static str,
    /// Share of the population. Used to prefer common machines, and shown in
    /// the interface so an operator can see when they are picking a rare one.
    weight: f64,
    gpu_model: &'static str,
    /// `None` keeps the base family's vendor string.
    gpu_vendor: Option<&'static str>,
    webgpu_arch: &'static str,
    width: u32,
    height: u32,
    cores: u32,
    memory_gb: u32,
    cpu_model: &'static str,
    /// `None` keeps the base OS version.
    os_version: Option<&'static str>,
}

/// Height lost to the Windows taskbar at 100% scaling.
const WINDOWS_TASKBAR: u32 = 48;
/// Height lost to the macOS menu bar plus the Dock, as measured.
const MACOS_CHROME: u32 = 123;

const MACOS: &[Variant] = &[
    Variant {
        id: "macos-m5-1470x956",
        weight: 0.012,
        gpu_model: "Apple M5",
        gpu_vendor: None,
        webgpu_arch: "metal-3",
        width: 1470,
        height: 956,
        cores: 10,
        memory_gb: 16,
        cpu_model: "Apple M-series",
        os_version: None,
    },
    Variant {
        id: "macos-m1-1440x900",
        weight: 0.041,
        gpu_model: "Apple M1",
        gpu_vendor: None,
        webgpu_arch: "metal-3",
        width: 1440,
        height: 900,
        cores: 8,
        memory_gb: 8,
        cpu_model: "Apple M-series",
        os_version: Some("14.7"),
    },
    Variant {
        id: "macos-m2-1512x982",
        weight: 0.034,
        gpu_model: "Apple M2",
        gpu_vendor: None,
        webgpu_arch: "metal-3",
        width: 1512,
        height: 982,
        cores: 8,
        memory_gb: 16,
        cpu_model: "Apple M-series",
        os_version: Some("15.1"),
    },
    Variant {
        id: "macos-m3-pro-1728x1117",
        weight: 0.019,
        gpu_model: "Apple M3 Pro",
        gpu_vendor: None,
        webgpu_arch: "metal-3",
        width: 1728,
        height: 1117,
        cores: 12,
        // Real M3 Pro machines ship with 18 GB, but navigator.deviceMemory is
        // quantised to a power of two before a page ever sees it, so what the
        // profile must carry is the reported value, not the physical one.
        memory_gb: 16,
        cpu_model: "Apple M-series",
        os_version: None,
    },
    Variant {
        id: "macos-m4-1710x1112",
        weight: 0.016,
        gpu_model: "Apple M4",
        gpu_vendor: None,
        webgpu_arch: "metal-3",
        width: 1710,
        height: 1112,
        cores: 10,
        memory_gb: 16,
        cpu_model: "Apple M-series",
        os_version: None,
    },
    Variant {
        id: "macos-m1pro-1512x982",
        weight: 0.011,
        gpu_model: "Apple M1 Pro",
        gpu_vendor: None,
        webgpu_arch: "metal-3",
        width: 1512,
        height: 982,
        cores: 10,
        memory_gb: 16,
        cpu_model: "Apple M-series",
        os_version: None,
    },
    Variant {
        id: "macos-m2pro-1728x1117",
        weight: 0.009,
        gpu_model: "Apple M2 Pro",
        gpu_vendor: None,
        webgpu_arch: "metal-3",
        width: 1728,
        height: 1117,
        cores: 12,
        memory_gb: 16,
        cpu_model: "Apple M-series",
        os_version: None,
    },
    Variant {
        id: "macos-m4pro-1728x1117",
        weight: 0.008,
        gpu_model: "Apple M4 Pro",
        gpu_vendor: None,
        webgpu_arch: "metal-3",
        width: 1728,
        height: 1117,
        cores: 12,
        // A real M4 Pro is commonly sold with 24 GB, and navigator.deviceMemory
        // cannot say so: the value is quantised to powers of two, so the machine
        // reports 16. The persona records what the browser reports, not what the
        // shop invoice said.
        memory_gb: 16,
        cpu_model: "Apple M-series",
        os_version: None,
    },
];

const WINDOWS: &[Variant] = &[
    Variant {
        id: "win11-rtx4060-1920x1080",
        weight: 0.031,
        gpu_model: "NVIDIA GeForce RTX 4060",
        gpu_vendor: None,
        webgpu_arch: "ada",
        width: 1920,
        height: 1080,
        cores: 12,
        memory_gb: 16,
        cpu_model: "Intel Core i5-13400F",
        os_version: None,
    },
    Variant {
        id: "win11-rtx3060-1920x1080",
        weight: 0.044,
        gpu_model: "NVIDIA GeForce RTX 3060",
        gpu_vendor: None,
        webgpu_arch: "ampere",
        width: 1920,
        height: 1080,
        cores: 12,
        memory_gb: 16,
        cpu_model: "Intel Core i5-12400F",
        os_version: None,
    },
    Variant {
        id: "win11-rtx4070-2560x1440",
        weight: 0.021,
        gpu_model: "NVIDIA GeForce RTX 4070",
        gpu_vendor: None,
        webgpu_arch: "ada",
        width: 2560,
        height: 1440,
        cores: 16,
        memory_gb: 32,
        cpu_model: "AMD Ryzen 7 7700X",
        os_version: None,
    },
    Variant {
        id: "win11-gtx1650-1920x1080",
        weight: 0.038,
        gpu_model: "NVIDIA GeForce GTX 1650",
        gpu_vendor: None,
        webgpu_arch: "turing",
        width: 1920,
        height: 1080,
        cores: 8,
        memory_gb: 8,
        cpu_model: "Intel Core i5-10400",
        os_version: None,
    },
    Variant {
        id: "win11-rtx3050-1366x768",
        weight: 0.024,
        gpu_model: "NVIDIA GeForce RTX 3050 Laptop GPU",
        gpu_vendor: None,
        webgpu_arch: "ampere",
        width: 1366,
        height: 768,
        cores: 8,
        memory_gb: 8,
        cpu_model: "Intel Core i5-11400H",
        os_version: None,
    },
    Variant {
        id: "win11-rx6600-1920x1080",
        weight: 0.023,
        gpu_model: "AMD Radeon RX 6600",
        gpu_vendor: Some("Google Inc. (AMD)"),
        webgpu_arch: "rdna-2",
        width: 1920,
        height: 1080,
        cores: 12,
        memory_gb: 16,
        cpu_model: "AMD Ryzen 5 5600",
        os_version: None,
    },
    Variant {
        id: "win11-iris-xe-1920x1080",
        weight: 0.052,
        gpu_model: "Intel(R) Iris(R) Xe Graphics",
        gpu_vendor: Some("Google Inc. (Intel)"),
        webgpu_arch: "gen-12lp",
        width: 1920,
        height: 1080,
        cores: 8,
        memory_gb: 16,
        cpu_model: "Intel Core i5-1235U",
        os_version: None,
    },
    Variant {
        id: "win11-uhd620-1366x768",
        weight: 0.029,
        gpu_model: "Intel(R) UHD Graphics 620",
        gpu_vendor: Some("Google Inc. (Intel)"),
        webgpu_arch: "gen-9",
        width: 1366,
        height: 768,
        cores: 8,
        memory_gb: 8,
        cpu_model: "Intel Core i5-8265U",
        os_version: None,
    },
    Variant {
        id: "win10-gtx1060-1920x1080",
        weight: 0.018,
        gpu_model: "NVIDIA GeForce GTX 1060 6GB",
        gpu_vendor: None,
        webgpu_arch: "pascal",
        width: 1920,
        height: 1080,
        cores: 6,
        memory_gb: 16,
        cpu_model: "Intel Core i5-9400F",
        os_version: Some("10"),
    },
    Variant {
        // Still the single most common discrete card in the Steam survey years
        // after release, and the reason a catalogue of current hardware is the
        // wrong shape: the crowd is old machines.
        id: "win11-gtx1660-1920x1080",
        weight: 0.035,
        gpu_model: "NVIDIA GeForce GTX 1660",
        gpu_vendor: None,
        webgpu_arch: "turing",
        width: 1920,
        height: 1080,
        cores: 6,
        memory_gb: 16,
        cpu_model: "Intel Core i5-13400F",
        os_version: None,
    },
    Variant {
        id: "win11-rtx3070-2560x1440",
        weight: 0.020,
        gpu_model: "NVIDIA GeForce RTX 3070",
        gpu_vendor: None,
        webgpu_arch: "ampere",
        width: 2560,
        height: 1440,
        cores: 8,
        memory_gb: 32,
        cpu_model: "Intel Core i5-13400F",
        os_version: None,
    },
    Variant {
        id: "win11-rtx2060-1920x1080",
        weight: 0.026,
        gpu_model: "NVIDIA GeForce RTX 2060",
        gpu_vendor: None,
        webgpu_arch: "turing",
        width: 1920,
        height: 1080,
        cores: 6,
        memory_gb: 16,
        cpu_model: "Intel Core i5-13400F",
        os_version: None,
    },
    Variant {
        id: "win11-rtx4060laptop-1920x1080",
        weight: 0.022,
        gpu_model: "NVIDIA GeForce RTX 4060 Laptop GPU",
        gpu_vendor: None,
        webgpu_arch: "ada",
        width: 1920,
        height: 1080,
        cores: 8,
        memory_gb: 16,
        cpu_model: "Intel Core i5-13400F",
        os_version: None,
    },
    Variant {
        id: "win11-rtx3060laptop-1920x1080",
        weight: 0.024,
        gpu_model: "NVIDIA GeForce RTX 3060 Laptop GPU",
        gpu_vendor: None,
        webgpu_arch: "ampere",
        width: 1920,
        height: 1080,
        cores: 8,
        memory_gb: 16,
        cpu_model: "Intel Core i5-13400F",
        os_version: None,
    },
    Variant {
        // The budget card that refuses to die, and an AMD entry that is not a
        // current generation — a catalogue of only new cards is its own tell.
        id: "win10-rx580-1920x1080",
        weight: 0.017,
        gpu_model: "AMD Radeon RX 580",
        gpu_vendor: Some("Google Inc. (AMD)"),
        webgpu_arch: "gcn-4",
        width: 1920,
        height: 1080,
        cores: 6,
        memory_gb: 16,
        cpu_model: "Intel Core i5-13400F",
        os_version: Some("10.0.0"),
    },
    Variant {
        id: "win11-rx7600-2560x1440",
        weight: 0.011,
        gpu_model: "AMD Radeon RX 7600",
        gpu_vendor: Some("Google Inc. (AMD)"),
        webgpu_arch: "rdna-3",
        width: 2560,
        height: 1440,
        cores: 12,
        memory_gb: 32,
        cpu_model: "Intel Core i5-13400F",
        os_version: None,
    },
    Variant {
        // The most common desktop integrated GPU there is, and 1536x864 is the
        // second most common laptop resolution after 1366x768 — both of them
        // machines nobody photographs but everybody uses.
        id: "win11-uhd630-1920x1080",
        weight: 0.041,
        gpu_model: "Intel(R) UHD Graphics 630",
        gpu_vendor: Some("Google Inc. (Intel)"),
        webgpu_arch: "gen-9",
        width: 1920,
        height: 1080,
        cores: 8,
        memory_gb: 16,
        cpu_model: "Intel Core i5-13400F",
        os_version: None,
    },
    Variant {
        id: "win11-iris-xe-1536x864",
        weight: 0.033,
        gpu_model: "Intel(R) Iris(R) Xe Graphics",
        gpu_vendor: Some("Google Inc. (Intel)"),
        webgpu_arch: "gen-12",
        width: 1536,
        height: 864,
        cores: 8,
        memory_gb: 8,
        cpu_model: "Intel Core i5-13400F",
        os_version: None,
    },
];

/// Every persona this build knows about.
/// Pick a persona in proportion to how common the machine is.
///
/// `roll` is any random `u64`; the same roll always gives the same persona, so
/// this is testable without an RNG and reproducible when a caller wants it.
///
/// # Why weighted rather than uniform, and why sharing is fine
///
/// Two hundred profiles created at once must look like two hundred people's
/// computers. Uniform choice would put as many accounts on the rarest machine
/// in the catalogue as on the commonest, which inverts the real distribution and
/// makes the rare ones a marker: a detector that sees an unusual configuration
/// twice in a week has learnt something, and it has learnt it from us.
///
/// Sharing a persona between profiles is not the linkability problem it sounds
/// like. A persona describes a machine millions of people own — that is the
/// point of the weights. What must never repeat is the profile's own seed,
/// which drives the canvas, audio and geometry noise, and its exit. Both of
/// those are per profile and neither comes from here.
pub fn pick_weighted(roll: u64) -> Persona {
    let all = all();
    // Weights are shares of the world's machines, so they sum to well under 1.
    // Normalising against their own total is what makes this a distribution
    // over the catalogue rather than over all computers — otherwise most rolls
    // would fall past the end and land on the last entry.
    let total: f64 = all.iter().map(|p| p.weight).sum();
    if total <= 0.0 {
        return all.into_iter().next().expect("the catalogue is never empty");
    }

    // 53 bits is the whole mantissa of an f64, so this is a uniform draw in
    // [0,1) with no rounding cliff at either end.
    let unit = (roll >> 11) as f64 / (1u64 << 53) as f64;
    let mut cursor = unit * total;
    for persona in &all {
        cursor -= persona.weight;
        if cursor < 0.0 {
            return persona.clone();
        }
    }
    // Only reachable through floating-point drift on the last subtraction.
    all.into_iter().next_back().expect("the catalogue is never empty")
}

pub fn all() -> Vec<Persona> {
    let mac: Persona = serde_json::from_str(MACOS_BASE).expect("macOS base persona parses");
    let win: Persona = serde_json::from_str(WINDOWS_BASE).expect("Windows base persona parses");

    let mut out = Vec::with_capacity(MACOS.len() + WINDOWS.len());
    for v in MACOS {
        out.push(expand(&mac, v, Family::MacOs));
    }
    for v in WINDOWS {
        out.push(expand(&win, v, Family::Windows));
    }
    out
}

enum Family {
    MacOs,
    Windows,
}

fn expand(base: &Persona, v: &Variant, family: Family) -> Persona {
    let mut p = base.clone();
    p.id = v.id.to_string();
    p.weight = v.weight;

    // Only the two bases were taken off a physical machine. Everything else is
    // that measurement with a documented model substituted, and must not claim
    // otherwise — "measured" is the stronger word and diluting it would make it
    // useless.
    p.source = Some(
        if v.id == "macos-m5-1470x956" || v.id == "win11-rtx4060-1920x1080" {
            "measured".to_string()
        } else {
            "derived".to_string()
        },
    );

    if let Some(version) = v.os_version {
        p.os.version = version.to_string();
        p.os.ch_platform_version = match family {
            Family::MacOs => format!("{version}.0"),
            // Windows reports a Client Hints platform version that has nothing
            // to do with the marketing name: 11 is 15.0.0, 10 is 10.0.0.
            Family::Windows if version == "10" => "10.0.0".to_string(),
            Family::Windows => "15.0.0".to_string(),
        };
    }

    p.gpu.webgl_renderer = match family {
        Family::MacOs => format!(
            "ANGLE (Apple, ANGLE Metal Renderer: {}, Unspecified Version)",
            v.gpu_model
        ),
        Family::Windows => format!(
            "ANGLE ({}, {} Direct3D11 vs_5_0 ps_5_0, D3D11)",
            windows_vendor_word(v.gpu_vendor),
            v.gpu_model
        ),
    };
    if let Some(vendor) = v.gpu_vendor {
        p.gpu.webgl_vendor = vendor.to_string();
    }
    // Derived from the vendor this persona ended up with, not from the
    // variant's optional override — defaulting to NVIDIA put an NVIDIA WebGPU
    // vendor behind an Apple Metal renderer, which the validator refused. That
    // refusal is the point of the validator.
    let webgpu_vendor_name = webgpu_vendor(&p.gpu.webgl_vendor).to_string();
    if let Some(webgpu) = p.gpu.webgpu.as_mut() {
        webgpu.architecture = v.webgpu_arch.to_string();
        webgpu.vendor = webgpu_vendor_name;
    }

    p.screen.width = v.width;
    p.screen.height = v.height;
    p.screen.avail_width = v.width;
    p.screen.avail_height = v.height
        - match family {
            Family::MacOs => MACOS_CHROME,
            Family::Windows => WINDOWS_TASKBAR,
        };

    p.cpu.cores = v.cores;
    p.cpu.model = Some(v.cpu_model.to_string());
    p.memory_gb = v.memory_gb;

    p
}

/// The word inside the ANGLE renderer string, which is not the same as the
/// WebGL vendor string wrapping it.
fn windows_vendor_word(vendor: Option<&str>) -> &'static str {
    match vendor {
        Some(v) if v.contains("AMD") => "AMD",
        Some(v) if v.contains("Intel") => "Intel",
        _ => "NVIDIA",
    }
}

fn webgpu_vendor(webgl_vendor: &str) -> &'static str {
    if webgl_vendor.contains("AMD") {
        "amd"
    } else if webgl_vendor.contains("Intel") {
        "intel"
    } else if webgl_vendor.contains("Apple") {
        "apple"
    } else {
        "nvidia"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_persona_in_the_catalogue_is_internally_consistent() {
        // The whole argument for a table instead of dropdowns: no entry here
        // can describe a machine that does not exist, and this is what enforces
        // it. Adding a row that contradicts itself fails the build.
        for p in all() {
            p.validate()
                .unwrap_or_else(|e| panic!("persona {}: {e:?}", p.id));
        }
    }

    /// The floor under a persona's share of the world.
    ///
    /// One in two hundred machines. Below that a persona stops being a crowd
    /// and starts being a marker: a detector that sees the same unusual
    /// configuration twice in a week has learnt that the two accounts are
    /// related, and it has learnt it from us. The catalogue's purpose is the
    /// opposite of a rare machine.
    ///
    /// This is a contribution gate, not a runtime filter. Nothing drops a
    /// persona at launch — a build with one below the floor simply does not
    /// pass, so the question is answered while somebody is still editing the
    /// table rather than after a profile has been used.
    const RARITY_FLOOR: f64 = 0.005;

    #[test]
    fn no_persona_is_rare_enough_to_be_a_marker() {
        for p in all() {
            assert!(
                p.weight >= RARITY_FLOOR,
                "persona {} has weight {}, under the {RARITY_FLOOR} floor — \
                 a machine that rare identifies whoever runs it. Either it is \
                 commoner than the weight says, or it does not belong in the \
                 catalogue.",
                p.id,
                p.weight,
            );
        }
    }

    #[test]
    fn the_catalogue_never_claims_more_of_the_world_than_exists() {
        // Weights are shares of all machines, so their sum is the share of the
        // world this catalogue covers, and it cannot exceed the world. It is
        // currently around a half, which is what 26 popular configurations
        // plausibly account for; a sum over 1 means somebody has been raising
        // a weight to make a persona get picked more often, which is what
        // pick_weighted's normalisation is for instead.
        let total: f64 = all().iter().map(|p| p.weight).sum();
        assert!(total <= 1.0, "weights sum to {total}, which is more machines than exist");
        assert!(total > 0.1, "weights sum to {total} — too little of the world to be a crowd");
    }

    #[test]
    fn identifiers_are_unique() {
        let mut ids: Vec<String> = all().into_iter().map(|p| p.id).collect();
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), before, "two personas share an id");
    }

    #[test]
    fn only_the_two_measured_machines_say_measured() {
        let measured: Vec<String> = all()
            .into_iter()
            .filter(|p| p.source.as_deref() == Some("measured"))
            .map(|p| p.id)
            .collect();
        assert_eq!(measured.len(), 2, "measured is a claim, not a default: {measured:?}");
    }

    #[test]
    fn a_screen_never_claims_more_usable_space_than_it_has() {
        for p in all() {
            assert!(
                p.screen.avail_height < p.screen.height,
                "{}: a window area equal to the screen means no taskbar and no menu bar, \
                 which is a configuration almost nobody runs",
                p.id
            );
            assert!(p.screen.avail_width <= p.screen.width, "{}", p.id);
        }
    }

    #[test]
    fn no_windows_gpu_appears_on_a_mac() {
        // The single most common impossible combination the dropdown-based
        // competition lets you build.
        for p in all() {
            let mac = p.os.name == "macOS";
            let nvidia = p.gpu.webgl_renderer.contains("NVIDIA");
            let metal = p.gpu.webgl_renderer.contains("Metal");
            assert!(!(mac && nvidia), "{}", p.id);
            assert!(!(!mac && metal), "{}", p.id);
        }
    }

    #[test]
    fn the_catalogue_is_wide_enough_to_hide_in() {
        // Two personas was the real complaint behind "they have far more
        // variety". Variety in machines, not in combinations.
        let all = all();
        assert!(all.len() >= 12, "only {} personas", all.len());
        let macs = all.iter().filter(|p| p.os.name == "macOS").count();
        assert!(macs >= 4 && all.len() - macs >= 6, "families are lopsided");
    }

    #[test]
    fn picking_follows_the_population_and_covers_the_catalogue() {
        // Deterministic sweep rather than an RNG: the same rolls always give
        // the same answer, so a regression here is reproducible.
        let all = all();
        let mut counts = std::collections::BTreeMap::new();
        let n = 20_000u64;
        for i in 0..n {
            // SplitMix64 over the index — a cheap way to get rolls that are
            // spread over the whole u64 range rather than clustered low.
            let mut z = i.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            let roll = z ^ (z >> 31);
            *counts.entry(pick_weighted(roll).id).or_insert(0u32) += 1;
        }

        // Every machine in the catalogue must be reachable. One that is never
        // picked is a machine nobody gets, which makes the catalogue a lie
        // about how much variety a bulk create produces.
        assert_eq!(counts.len(), all.len(), "some personas were never picked: {counts:?}");

        // And the shares must track the weights. The commonest machine must
        // come up more often than the rarest, or the distribution is inverted
        // and rare configurations become a marker.
        let total: f64 = all.iter().map(|p| p.weight).sum();
        for p in &all {
            let got = counts[&p.id] as f64 / n as f64;
            let want = p.weight / total;
            assert!(
                (got - want).abs() < 0.01,
                "{}: picked {:.3} of the time, population share is {:.3}",
                p.id,
                got,
                want
            );
        }
    }

    #[test]
    fn the_same_roll_always_gives_the_same_machine() {
        for roll in [0u64, 1, u64::MAX, u64::MAX / 2, 0x0123_4567_89ab_cdef] {
            assert_eq!(pick_weighted(roll).id, pick_weighted(roll).id);
        }
        // Both ends of the range land on a real persona rather than falling off
        // the loop — the two cases a cumulative walk gets wrong.
        assert!(!pick_weighted(0).id.is_empty());
        assert!(!pick_weighted(u64::MAX).id.is_empty());
    }
}
