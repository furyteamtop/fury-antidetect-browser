// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The persona catalogue.
//!
//! Built in rather than loaded from disk, so a fresh installation can create a
//! working profile with no setup step. A persona is one real machine's measured
//! configuration; shipping them inside the binary also means an operator cannot
//! accidentally edit one into an impossible device, which would be a detection
//! signal rather than a broken feature.
//!
//! `FURY_PERSONAS` points at a directory of extra `.json` files for anyone
//! building their own — they are added to the built-ins, never replace them.

use fury_shared::persona::Persona;
use serde::Serialize;

/// The chip this Mac calls itself ("Apple M5", "Apple M3 Max"), read once
/// from `machdep.cpu.brand_string`. On Apple silicon it is also the Metal
/// device's name, which is what Chrome puts in the WebGL renderer: checked
/// 28.11.2026 on an M5, real Chrome 158 says "ANGLE Metal Renderer: Apple M5".
/// None off macOS, and on an Intel Mac, whose brand string is the CPU's.
pub fn host_chip() -> Option<&'static str> {
    static CHIP: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    CHIP.get_or_init(|| {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let out = std::process::Command::new("/usr/sbin/sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()?;
        let raw = String::from_utf8(out.stdout).ok()?;
        let chip = raw
            .trim()
            .strip_suffix(" (Virtual)")
            .unwrap_or(raw.trim())
            .trim()
            .to_string();
        chip.starts_with("Apple ").then_some(chip)
    })
    .as_deref()
}

/// A macOS persona on this Mac names this Mac's chip and reports this Mac's
/// core count, unless the profile pinned a count of its own. See
/// `Persona::name_the_host_chip` for the chip (issue #56).
///
/// The cores, measured the same day on the M5 (10 cores), playground visits
/// with the chip already the host's: personas reporting another count were
/// flagged 6 of 19 (M2's 8 cores: 5 of 15), the ones reporting 10 were 2 of 26.
/// A page can count cores by timing workers, so the claim has to be the real
/// one. An odd count (an 11-core M3 Pro) fails `validate` and is left as the
/// persona has it.
///
/// Every place that turns a persona into a core config goes through here, so
/// the preview shows what the launch will claim.
pub fn fit_to_host(p: &mut Persona, cores_pinned: bool) {
    let Some(chip) = host_chip() else { return };
    if p.os.name != "macOS" {
        return;
    }
    p.name_the_host_chip(chip);
    if !cores_pinned {
        if let Some(n) = std::thread::available_parallelism().ok().map(|n| n.get() as u32) {
            if n % 2 == 0 && n <= 128 {
                p.cpu.cores = n;
            }
        }
    }
}

/// What the interface shows in a persona picker.
#[derive(Debug, Serialize)]
pub struct PersonaSummary {
    pub id: String,
    pub os: String,
    pub gpu: String,
    pub screen: String,
    /// Share of the real population. An operator choosing between two personas
    /// should prefer the common one; a rare-but-real device narrows the crowd
    /// they hide in.
    pub weight: f64,
    pub source: Option<String>,
    /// Not built in: captured on this computer, fetched from the team, or from
    /// FURY_PERSONAS. The picker lists these apart, above the catalogue, where
    /// a tester expected his own machines to be (13.10.2026).
    pub own: bool,
}

pub fn catalogue() -> Vec<PersonaSummary> {
    let builtin: Vec<String> = fury_shared::catalogue::all().into_iter().map(|p| p.id).collect();
    let mut personas = all();
    // Commonest first. Sorting here rather than in the table keeps the table
    // grouped by family, which is how it is read and edited.
    personas.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    personas
        .into_iter()
        .map(|mut p| {
            fit_to_host(&mut p, false);
            p
        })
        .map(|p| PersonaSummary {
            os: format!("{} {}", p.os.name, p.os.display_version()),
            gpu: p.gpu.webgl_renderer.clone(),
            screen: format!("{}×{}", p.screen.width, p.screen.height),
            weight: p.weight,
            source: p.source.clone(),
            own: !builtin.contains(&p.id),
            id: p.id,
        })
        .collect()
}

pub fn all() -> Vec<Persona> {
    // The catalogue lives in the shared crate, where a test refuses to build if
    // any entry describes a machine that cannot exist.
    let mut out: Vec<Persona> = fury_shared::catalogue::all();

    // Two places add to it, never replace it: the folder a capture is kept in
    // when "Use on this computer" is pressed, and FURY_PERSONAS for anyone
    // building their own. The folder came first in a tester's question -- "I
    // took a capture of my machine, how do I use it?" -- and the honest answer
    // then was an environment variable and a reboot.
    load_dir(&crate::paths::personas_dir(), &mut out);
    if let Ok(dir) = std::env::var("FURY_PERSONAS") {
        load_dir(std::path::Path::new(&dir), &mut out);
    }

    out
}

fn load_dir(dir: &std::path::Path, out: &mut Vec<Persona>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            match std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<Persona>(&raw).ok())
            {
                Some(p) if !out.iter().any(|b| b.id == p.id) => out.push(p),
                Some(_) => tracing::warn!(?path, "persona id already present — ignored"),
                None => tracing::warn!(?path, "persona does not parse — ignored"),
            }
        }
    }
}

pub fn load(id: &str) -> anyhow::Result<Persona> {
    all()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow::anyhow!("no persona {id:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_persona_is_consistent() {
        let all = all();
        assert!(all.len() >= 12, "only {} personas", all.len());
        for p in &all {
            p.validate()
                .unwrap_or_else(|e| panic!("persona {}: {e:?}", p.id));
        }
    }

    #[test]
    fn the_catalogue_is_sorted_so_common_machines_come_first() {
        // An operator scanning the list should meet the ordinary machines
        // before the rare ones; a rare-but-real device narrows the crowd.
        let weights: Vec<f64> = catalogue().into_iter().map(|p| p.weight).collect();
        assert!(
            weights.windows(2).all(|w| w[0] >= w[1]),
            "catalogue is not ordered by population share: {weights:?}"
        );
    }

    #[test]
    fn the_catalogue_describes_a_device_a_person_can_choose_between() {
        for entry in catalogue() {
            assert!(!entry.os.trim().is_empty());
            assert!(!entry.gpu.trim().is_empty());
            assert!(entry.screen.contains('×'));
            assert!(entry.weight > 0.0);
        }
    }

    #[test]
    fn on_this_mac_a_mac_persona_reports_this_macs_chip_and_cores() {
        let Some(chip) = host_chip() else {
            eprintln!("SKIPPED: not an Apple silicon Mac");
            return;
        };
        let cores = std::thread::available_parallelism().unwrap().get() as u32;
        for mut p in all() {
            let was = p.clone();
            fit_to_host(&mut p, false);
            p.validate().unwrap_or_else(|e| panic!("{}: {e:?}", p.id));
            if p.os.name == "macOS" {
                assert!(p.gpu.webgl_renderer.contains(&format!("Metal Renderer: {chip},")), "{}", p.gpu.webgl_renderer);
                if cores % 2 == 0 {
                    assert_eq!(p.cpu.cores, cores, "{}", p.id);
                }
                let mut pinned = was.clone();
                fit_to_host(&mut pinned, true);
                assert_eq!(pinned.cpu.cores, was.cpu.cores, "a pinned count stays");
            } else {
                assert_eq!(serde_json::to_value(&p).unwrap(), serde_json::to_value(&was).unwrap(), "{}", p.id);
            }
        }
    }

    #[test]
    fn an_unknown_persona_is_an_error_not_a_default() {
        // Falling back to some other device would give a profile a fingerprint
        // its owner never chose, and they would not find out.
        assert!(load("no-such-persona").is_err());
    }
}
