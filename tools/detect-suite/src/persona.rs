// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//! `fury-detect persona`: the command-line face of `fury_shared::capture`.
//! The conversion itself moved to shared-rs on 12.09.2026 so the agent can
//! run it for the desktop's "capture this machine" button.
use anyhow::{Context, Result};
use fury_shared::capture::{from_capture, n, s};
use serde_json::Value;

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
            // "(TM)" sits between a phone GPU's family and its number:
            // "Adreno (TM) 722".
            let words: Vec<&str> = flat.split_whitespace().filter(|w| *w != "TM").collect();
            let at = words.iter().position(|w| {
                ["RTX", "GTX", "Radeon", "Iris", "UHD", "Adreno", "Mali", "PowerVR"].contains(w)
                    // "Mali-G76": family and model in one word.
                    || w.starts_with("Mali-")
                    || (*w == "M5" || *w == "M4" || *w == "M3" || *w == "M2" || *w == "M1")
            });
            match at {
                Some(i) if words[i].starts_with("Mali-") => words[i].to_lowercase(),
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

    let persona = from_capture(&dump, &id, weight).map_err(anyhow::Error::msg)?;

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


/// `fury-detect personas-check [dir]`: what CI runs on a pull request that
/// adds a contributed persona, and what a contributor can run first.
///
/// Every file in the directory must: parse as a persona; pass `validate()`;
/// carry an id equal to its file name and unique against the whole catalogue;
/// say `source: "capture"` — "measured" is reserved for the two machines the
/// maintainers dumped and checked by hand, and a contribution that claims it
/// is refused for the word, not the data; and carry a weight above the
/// catalogue's rarity floor, since a persona below it is never picked and a
/// file nobody is ever handed is a file that should not be merged.
pub fn cmd_personas_check(args: &[String]) -> Result<()> {
    let dir = args
        .first()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("shared/personas/contributed"));
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();

    // Ids already shipped, minus the contributed ones (they are what is being
    // checked) — so a contributed file is compared against the built-in set
    // and against its siblings, not against itself.
    let builtin: std::collections::HashSet<String> = fury_shared::catalogue::all()
        .into_iter()
        .filter(|p| p.source.as_deref() != Some("capture"))
        .map(|p| p.id)
        .collect();
    let mut seen: std::collections::HashSet<String> = Default::default();
    let mut failures = 0;

    for f in &files {
        let name = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let mut problems: Vec<String> = Vec::new();
        let text = std::fs::read_to_string(f)?;
        match serde_json::from_str::<fury_shared::persona::Persona>(&text) {
            Err(e) => problems.push(format!("does not parse as a persona: {e}")),
            Ok(p) => {
                if p.id != name {
                    problems.push(format!("id {:?} does not match the file name {name:?}", p.id));
                }
                if builtin.contains(&p.id) || !seen.insert(p.id.clone()) {
                    problems.push(format!("id {:?} is already in the catalogue", p.id));
                }
                if p.source.as_deref() != Some("capture") {
                    problems.push(format!(
                        "source is {:?}; a contributed persona says \"capture\" — \"measured\" is reserved",
                        p.source
                    ));
                }
                if p.weight < 0.005 {
                    problems.push(format!("weight {} is below the rarity floor and would never be picked", p.weight));
                }
                if let Err(errs) = p.validate() {
                    for e in errs {
                        problems.push(format!("validate(): {e}"));
                    }
                }
            }
        }
        // Nothing network-shaped belongs in a persona at all.
        for key in ["ip", "webrtc", "addresses", "publicIp", "hostname"] {
            if text.contains(&format!("\"{key}\"")) {
                problems.push(format!("carries a {key:?} field — a persona describes hardware, never a network"));
            }
        }
        if problems.is_empty() {
            println!("ok   {name}");
        } else {
            failures += 1;
            println!("FAIL {name}");
            for p in problems {
                println!("       {p}");
            }
        }
    }
    println!("{} contributed personas, {} failing", files.len(), failures);
    if failures > 0 {
        anyhow::bail!("{failures} contributed persona(s) need fixing");
    }
    Ok(())
}
