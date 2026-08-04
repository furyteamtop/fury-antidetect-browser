// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Putting a downloaded core where the agent will find it.
//!
//! The browser is not in the application bundle. It is 134 MB compressed
//! against the shell's 12 MB, it is versioned on its own schedule, and writing
//! it into a signed bundle would invalidate that bundle's signature — after
//! which macOS refuses to launch it and says the application is damaged, which
//! sends the user to look for a corrupt download rather than for us.
//!
//! So it arrives as a separate file and this puts it in `paths::core_dir()`,
//! which [`crate::core_binary`] looks in.
//!
//! Deliberately takes a path, not a URL. Downloading and then executing is the
//! shape of every supply-chain compromise there is, and the person running this
//! should have got the file the way they get any other download, where their
//! browser told them where it came from. What this does instead is the part
//! that is genuinely awkward by hand: unpack, strip the quarantine flag, and —
//! the step everyone forgets — actually run the thing once and check it starts,
//! because an unpacked bundle that will not launch looks exactly like an
//! installed one until the first profile fails.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Moves a core installed by an earlier version into the current directory.
///
/// Cheap enough to call on every lookup — two `exists()` calls when there is
/// nothing to do — and it has to be, because the alternative is a user whose
/// browser quietly stops being found the day they update the application.
///
/// A rename, not a copy: the core is 544 MB and both are on the same volume.
pub fn migrate_legacy_dir() {
    let (old, new) = (crate::paths::legacy_core_dir(), crate::paths::core_dir());
    if !old.is_dir() || new.exists() {
        return;
    }
    match std::fs::rename(&old, &new) {
        Ok(()) => tracing::info!(
            from = %old.display(), to = %new.display(),
            "moved the installed core into a package so search shows one Fury"
        ),
        // Not fatal. The next install-core writes the new location anyway, and
        // failing to start over a directory name would be a poor trade.
        Err(e) => tracing::warn!(error = %e, "could not move {}", old.display()),
    }
}

/// Installs a core from `src`, which may be a `.tar.xz`/`.tar.gz` archive, a
/// `.app` bundle, or the directory one was unpacked into.
pub fn install(src: &Path) -> Result<PathBuf> {
    if !src.exists() {
        bail!("{} does not exist", src.display());
    }

    let dest = crate::paths::core_dir();
    let staging = dest.with_extension("incoming");

    // A failed install must not eat the working core. Everything lands beside
    // the real directory and only replaces it once it has been checked.
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .with_context(|| format!("creating {}", staging.display()))?;

    if src.is_dir() {
        // `cp -R` on macOS, a recursive walk on Windows — and the reason
        // they differ is symlinks. See fury_platform::tree.
        fury_platform::copy_tree(src, &staging)?;
    } else {
        unpack(src, &staging)?;
    }

    // An archive may or may not have a top-level directory, and a copied .app
    // lands as a child. Find the executable rather than assuming the shape.
    let leaf = find_core(&staging)
        .ok_or_else(|| anyhow::anyhow!(
            "no core found in {} — expected {} somewhere inside it",
            src.display(),
            super::core_leaf(),
        ))?;

    // The quarantine flag is set by the browser that downloaded the file and is
    // inherited by everything unpacked from it. Left on, macOS blocks the
    // launch with a dialog naming the *helper* process, which is the least
    // actionable message in the operating system.
    #[cfg(target_os = "macos")]
    unquarantine(&staging);

    ensure_executable(&leaf)?;
    let version = probe_version(&leaf)?;


    // Only now is the old one worth losing.
    let previous = dest.with_extension("previous");
    let _ = std::fs::remove_dir_all(&previous);
    if dest.exists() {
        std::fs::rename(&dest, &previous)
            .with_context(|| format!("moving the existing core aside from {}", dest.display()))?;
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&staging, &dest).with_context(|| {
        format!("moving the new core into {}", dest.display())
    })?;
    let _ = std::fs::remove_dir_all(&previous);

    // Where core_binary() will look, which is not necessarily where it landed:
    // the staging tree keeps its own shape and the rename moves the whole thing.
    let installed = dest.join(super::core_leaf());
    let installed = if installed.exists() {
        installed
    } else {
        find_core(&dest).unwrap_or(installed)
    };

    eprintln!("installed {version}");
    eprintln!("  {}", installed.display());
    Ok(installed)
}

/// Reports what is installed, without changing anything.
pub fn status() -> Result<()> {
    match crate::core_binary() {
        Some(path) => {
            let version = probe_version(&path).unwrap_or_else(|e| format!("(will not run: {e})"));
            println!("{version}");
            println!("  {}", path.display());
            // Only when the path actually came from the variable. Saying
            // "(from FURY_CORE)" whenever it is merely SET was wrong the moment
            // a stale one started being ignored: it credited the variable for a
            // core found in spite of it.
            if std::env::var("FURY_CORE").is_ok_and(|v| std::path::Path::new(&v) == path) {
                println!("  (from FURY_CORE)");
            }
            // A core was found and something is still wrong — a leftover
            // variable naming a deleted build. Worth saying while somebody is
            // looking, rather than the next time it costs them an hour.
            if let Some(why) = crate::core_lookup_problem() {
                println!();
                println!("note: {why}");
            }
            Ok(())
        }
        None => {
            println!("no core installed");
            match crate::core_lookup_problem() {
                Some(why) => println!("  {why}"),
                None => {
                    println!(
                        "  looked beside the agent, and in {}",
                        crate::paths::core_dir().display()
                    );
                    println!("  install one with: fury-agent install-core <file>");
                }
            }
            Ok(())
        }
    }
}

/// Runs the binary with `--version` and returns what it said.
///
/// The check that separates "unpacked" from "installed". A bundle with a
/// missing framework, a wrong architecture or a signature macOS rejects all
/// unpack perfectly and all fail here, in two seconds, next to the command that
/// caused them — rather than an hour later when a profile will not start.
fn probe_version(exe: &Path) -> Result<String> {
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .with_context(|| format!("running {}", exe.display()))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        // Deliberately not "run codesign --deep --sign -". That is the usual
        // advice and it is wrong for this bundle: --deep gives every nested
        // helper the outer app's entitlements, and the renderer, GPU and
        // plugin helpers each need different ones. It produces something that
        // launches and then fails in ways that look like anything but signing.
        let hint = if err.contains("different Team IDs") {
            "\nThis build was signed ad-hoc with library validation on, which \
             requires a Team ID that an ad-hoc signature does not have. It needs \
             signing with a Developer ID — see tools/release/sign-core.sh."
        } else if err.contains("code signature") || err.contains("Killed") {
            "\nmacOS rejected the signature. Do not reach for `codesign --deep` \
             — Chrome's helpers each need their own entitlements. \
             tools/release/sign-core.sh drives Chromium's own pipeline."
        } else {
            ""
        };
        bail!("the core does not start: {}{hint}", err.trim());
    }

    let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if said.is_empty() {
        bail!("the core started but reported no version");
    }
    Ok(said)
}

fn unpack(archive: &Path, into: &Path) -> Result<()> {
    let name = archive.to_string_lossy().to_lowercase();
    if !(name.ends_with(".tar.xz")
        || name.ends_with(".tar.gz")
        || name.ends_with(".tgz")
        || name.ends_with(".tar")
        || name.ends_with(".zip"))
    {
        bail!(
            "{} is not an archive this understands (.tar.xz, .tar.gz, .tar, .zip), \
             a .app bundle, or a directory",
            archive.display()
        );
    }

    // Shelling out to tar rather than linking a decompressor: tar preserves the
    // symlinks a macOS framework is built from and the executable bits, and
    // getting either wrong produces a bundle that unpacks and will not run.
    //
    // The same command covers .zip, which is how Chromium is packaged for
    // Windows. `tar` on both platforms is bsdtar/libarchive — Windows has
    // shipped it since 10 build 17063 — and it reads zip as readily as tar.
    // Measured rather than assumed: a zip made here, extracted with `tar -xf`,
    // came out with its subdirectories and its executable bit intact.
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .context("running tar")?;
    if !status.success() {
        bail!("tar could not unpack {}", archive.display());
    }
    Ok(())
}

/// Finds the core executable anywhere in a freshly unpacked tree.
///
/// Bounded rather than a full walk: an archive is either flat or has one
/// wrapping directory, and descending further would find helper executables
/// with the same name inside the framework.
fn find_core(root: &Path) -> Option<PathBuf> {
    let leaf = super::core_leaf();
    let direct = root.join(leaf);
    if direct.exists() {
        return Some(direct);
    }
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let candidate = entry.path().join(leaf);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn ensure_executable(exe: &Path) -> Result<()> {
    // A no-op on Windows, where the extension decides. See
    // fury_platform::perms::make_executable.
    fury_platform::perms::make_executable(exe)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn unquarantine(tree: &Path) {
    // Best effort. If xattr is missing or the flag was never set, the failure
    // is not one the user needs to hear about — and if it mattered, the version
    // probe two lines later fails with the real message.
    let _ = std::process::Command::new("xattr")
        .arg("-dr")
        .arg("com.apple.quarantine")
        .arg(tree)
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_shape_it_cannot_read_is_named_rather_than_attempted() {
        let dir = std::env::temp_dir().join("fury-install-core-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dmg = dir.join("core.dmg");
        std::fs::write(&dmg, b"not really a dmg").unwrap();

        let err = unpack(&dmg, &dir).unwrap_err().to_string();
        assert!(err.contains(".tar.xz"), "{err}");
        // The point of the check: tar never ran, so there is no half-unpacked
        // directory to explain afterwards.
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_source_is_refused_before_anything_is_moved() {
        let err = install(Path::new("/nonexistent/fury-core.tar.xz"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn the_core_is_found_flat_or_one_level_down() {
        let root = std::env::temp_dir().join("fury-find-core-test");
        std::fs::remove_dir_all(&root).ok();

        // An archive with a wrapping directory, which is the commoner shape.
        let nested = root.join("fury-core-150.0.7871.187");
        let leaf = nested.join(super::super::core_leaf());
        std::fs::create_dir_all(leaf.parent().unwrap()).unwrap();
        std::fs::write(&leaf, b"#!/bin/sh\n").unwrap();
        assert_eq!(find_core(&root), Some(leaf));

        // And one without.
        let flat = root.join("flat");
        let flat_leaf = flat.join(super::super::core_leaf());
        std::fs::create_dir_all(flat_leaf.parent().unwrap()).unwrap();
        std::fs::write(&flat_leaf, b"#!/bin/sh\n").unwrap();
        assert_eq!(find_core(&flat), Some(flat_leaf));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_binary_that_will_not_run_is_not_an_installed_core() {
        // The whole reason the version probe exists. This file unpacks
        // perfectly and is not a browser.
        let dir = std::env::temp_dir().join("fury-probe-test");
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("not-a-browser");
        std::fs::write(&fake, b"#!/bin/sh\nexit 1\n").unwrap();
        ensure_executable(&fake).unwrap();

        let err = probe_version(&fake).unwrap_err().to_string();
        assert!(err.contains("does not start"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
