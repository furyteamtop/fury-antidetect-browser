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
}

pub fn catalogue() -> Vec<PersonaSummary> {
    let mut personas = all();
    // Commonest first. Sorting here rather than in the table keeps the table
    // grouped by family, which is how it is read and edited.
    personas.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    personas
        .into_iter()
        .map(|p| PersonaSummary {
            os: format!("{} {}", p.os.name, p.os.version),
            gpu: p.gpu.webgl_renderer.clone(),
            screen: format!("{}×{}", p.screen.width, p.screen.height),
            weight: p.weight,
            source: p.source.clone(),
            id: p.id,
        })
        .collect()
}

pub fn all() -> Vec<Persona> {
    // The catalogue lives in the shared crate, where a test refuses to build if
    // any entry describes a machine that cannot exist.
    let mut out: Vec<Persona> = fury_shared::catalogue::all();

    if let Ok(dir) = std::env::var("FURY_PERSONAS") {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    match std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|raw| serde_json::from_str::<Persona>(&raw).ok())
                    {
                        Some(p) if !out.iter().any(|b| b.id == p.id) => out.push(p),
                        Some(_) => tracing::warn!(?path, "persona id already built in — ignored"),
                        None => tracing::warn!(?path, "persona does not parse — ignored"),
                    }
                }
            }
        }
    }

    out
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
    fn an_unknown_persona_is_an_error_not_a_default() {
        // Falling back to some other device would give a profile a fingerprint
        // its owner never chose, and they would not find out.
        assert!(load("no-such-persona").is_err());
    }
}
