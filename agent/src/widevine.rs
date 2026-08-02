// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Staging the Widevine CDM from the Chrome the user already has.
//!
//! # The problem this exists for
//!
//! Real Chrome accepts `requestMediaKeySystemAccess("com.widevine.alpha")`. A
//! browser that refuses it is not Chrome, and saying so takes two lines of
//! JavaScript — which makes it a detection, not just a missing feature. It is
//! also the difference between a profile that can open Netflix and one that
//! cannot.
//!
//! The CDM that answers that call is a proprietary binary. Its own licence
//! (third_party/widevine/cdm/mac/*/LICENSE) says it may not be used, modified,
//! sold or distributed without a separate agreement with Google, and Chromium's
//! own annotation records it as approved for Chrome and not for Chromium. So
//! Fury cannot ship it: not in the repository, not in a release, not in an
//! installer.
//!
//! What Fury CAN do is notice that the user already has one. Anybody running
//! Chrome on this machine has a licensed copy sitting in the Chrome bundle. This
//! copies it into Fury's own directory so the browser can load it. Nothing
//! leaves the machine and nothing is redistributed — the same argument
//! core/build/link-widevine.sh makes for a developer's build, applied at the
//! other end, for a user's install.
//!
//! # Why it is not enough to have the file
//!
//! Chromium registers the bundled CDM through the component updater's
//! registration path. Measured 02.08.2026: with `--disable-component-update` the
//! blob sits in the bundle and nothing ever tells the browser it is there, and
//! `requestMediaKeySystemAccess` refuses. That switch is gone from the launcher
//! for exactly this reason — see the note in `launcher::build_args`. Staging the
//! file is half the job; not disabling the registration path is the other half.

use std::path::{Path, PathBuf};

/// Where a Chrome install keeps its CDM, and where ours goes.
#[derive(Debug)]
pub struct Staged {
    pub from: PathBuf,
    pub to: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("no Google Chrome found — looked in {0}")]
    NoChrome(String),
    #[error(
        "Chrome is installed but its Widevine CDM is not where it should be ({0}). \
         Opening a DRM video in Chrome once usually puts it there."
    )]
    NoCdm(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The platform subdirectory Chromium looks in.
fn platform_dir() -> &'static str {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") { "mac_arm64" } else { "mac_x64" }
    } else if cfg!(target_os = "windows") {
        if cfg!(target_arch = "x86_64") { "win_x64" } else { "win_x86" }
    } else if cfg!(target_arch = "x86_64") {
        "linux_x64"
    } else {
        "linux_arm64"
    }
}

fn library_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libwidevinecdm.dylib"
    } else if cfg!(target_os = "windows") {
        "widevinecdm.dll"
    } else {
        "libwidevinecdm.so"
    }
}

/// Everywhere a Chrome install might be on this platform.
///
/// Deliberately not configurable by the profile. This reads a file off the
/// user's own disk on their behalf; the set of places it will look is fixed and
/// visible in the source, rather than something a synced profile could point
/// somewhere interesting.
fn chrome_candidates() -> Vec<PathBuf> {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).ok();
    let mut out: Vec<PathBuf> = Vec::new();
    if cfg!(target_os = "macos") {
        out.push("/Applications/Google Chrome.app".into());
        out.push("/Applications/Google Chrome Beta.app".into());
        if let Some(h) = &home {
            out.push(Path::new(h).join("Applications/Google Chrome.app"));
        }
    } else if cfg!(target_os = "windows") {
        for base in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Ok(b) = std::env::var(base) {
                out.push(Path::new(&b).join(r"Google\Chrome\Application"));
            }
        }
    } else {
        out.push("/opt/google/chrome".into());
        out.push("/usr/lib/google-chrome".into());
    }
    out
}

/// Find the CDM inside a Chrome install, whatever version it is.
///
/// Chrome keeps it under a version directory that will not match ours, and it
/// does not need to: the CDM has its own interface version and Chromium loads
/// whatever it finds. Matching versions is what the BUILD-time script does
/// because it is compiling against one; at runtime we take what is there.
fn find_cdm(chrome: &Path) -> Option<PathBuf> {
    let versions = if cfg!(target_os = "macos") {
        chrome.join("Contents/Frameworks/Google Chrome Framework.framework/Versions")
    } else {
        chrome.to_path_buf()
    };
    let mut found: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(&versions).ok()?;
    for entry in entries.flatten() {
        let dir = if cfg!(target_os = "macos") {
            entry.path().join("Libraries/WidevineCdm")
        } else {
            entry.path().join("WidevineCdm")
        };
        let lib = dir.join("_platform_specific").join(platform_dir()).join(library_name());
        if lib.is_file() {
            found.push(lib);
        }
    }
    // Newest version directory last, so take the last one sorted.
    found.sort();
    found.pop()
}

/// Copy the CDM out of the user's Chrome into `dest`, unless it is already
/// there and the same size.
///
/// `dest` is the WidevineCdm directory beside the Fury core — the same layout
/// Chromium's bundled CDM uses, so no switch or preference is needed to find it.
pub fn stage(dest_root: &Path) -> Result<Staged, StageError> {
    let candidates = chrome_candidates();
    let chrome = candidates
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| {
            StageError::NoChrome(
                candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
            )
        })?;

    let from = find_cdm(chrome).ok_or_else(|| StageError::NoCdm(chrome.clone()))?;
    let src_dir = from.parent().unwrap();

    let to_dir = dest_root.join("_platform_specific").join(platform_dir());
    std::fs::create_dir_all(&to_dir)?;
    let to = to_dir.join(library_name());

    let bytes = std::fs::metadata(&from)?.len();
    let already = std::fs::metadata(&to).map(|m| m.len()).unwrap_or(0);
    if already == bytes {
        return Ok(Staged { from, to, bytes });
    }

    std::fs::copy(&from, &to)?;

    // The signature file and the manifest travel with it. Chromium checks the
    // manifest for the CDM's interface version, and a library without one is a
    // library it will not load.
    for name in ["libwidevinecdm.dylib.sig", "widevinecdm.dll.sig", "libwidevinecdm.so.sig"] {
        let sig = src_dir.join(name);
        if sig.is_file() {
            let _ = std::fs::copy(&sig, to_dir.join(name));
        }
    }
    // manifest.json is TWO levels up, not one: the library lives in
    // <WidevineCdm>/_platform_specific/<platform>/, so src_dir.parent() is
    // _platform_specific and the manifest is its parent again.
    //
    // The first version got this wrong and the failure was silent in exactly the
    // way that matters: the copy succeeded, the log said "staged the Widevine
    // CDM", and the browser answered requestMediaKeySystemAccess with
    // NotSupportedError anyway. Chromium reads the manifest for the CDM's
    // interface version and codec list, and a library without one is a library
    // it will not register. Measured, then fixed, then measured again.
    let cdm_root = src_dir.parent().and_then(|p| p.parent());
    if let Some(root) = cdm_root {
        for name in ["manifest.json", "LICENSE"] {
            let from = root.join(name);
            if from.is_file() {
                std::fs::copy(&from, dest_root.join(name))?;
            }
        }
    }
    // Without the manifest the rest is useless, so say so rather than report a
    // success the browser will not honour.
    if !dest_root.join("manifest.json").is_file() {
        return Err(StageError::NoCdm(chrome.clone()));
    }

    Ok(Staged { from, to, bytes })
}

/// Where the CDM belongs for a given core binary.
///
/// Mirrors the layout `bundle_widevine_cdm` produces, so a build that bundled
/// its own and an install that staged one look identical to the browser.
pub fn destination_for(core: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        // …/Fury.app/Contents/MacOS/Fury -> …/Fury.app/Contents/Frameworks/
        //   Fury Framework.framework/Versions/<v>/Libraries/WidevineCdm
        let contents = core.parent()?.parent()?;
        let frameworks = contents.join("Frameworks");
        let fw = std::fs::read_dir(&frameworks)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "framework"))?;
        let versions = fw.join("Versions");
        let version = std::fs::read_dir(&versions)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n != "Current"))
            .max()?;
        Some(version.join("Libraries/WidevineCdm"))
    } else {
        Some(core.parent()?.join("WidevineCdm"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_directory_is_the_one_chromium_looks_in() {
        // These strings are Chromium's, not ours — components/cdm/common has the
        // same table. Getting one wrong stages the file somewhere the browser
        // never looks, which fails exactly like not staging it at all.
        let d = platform_dir();
        assert!(
            ["mac_arm64", "mac_x64", "win_x64", "win_x86", "linux_x64", "linux_arm64"]
                .contains(&d),
            "unexpected platform dir {d}"
        );
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(d, "mac_arm64");
    }

    #[test]
    fn a_missing_chrome_is_an_error_that_says_where_it_looked() {
        // The failure a user actually hits, and the message is the whole
        // remedy: "no CDM" tells them nothing, "looked in /Applications/Google
        // Chrome.app" tells them to install Chrome or say where it is.
        let err = StageError::NoChrome("/Applications/Google Chrome.app".into());
        let text = err.to_string();
        assert!(text.contains("/Applications/Google Chrome.app"), "{text}");
    }

    #[test]
    fn the_manifest_travels_with_the_library() {
        // The bug this pins, in the shape it shipped in: the manifest lives two
        // directories above the library, not one, and copying only the library
        // produced a staging that logged success and left the browser answering
        // NotSupportedError. Chromium reads the manifest for the CDM's interface
        // version; without it there is nothing to register.
        //
        // Skipped rather than failed where there is no Chrome — this asserts a
        // property of the copy, and a CI machine without Chrome has nothing to
        // copy.
        let tmp = std::env::temp_dir().join(format!("fury-wv-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        match stage(&tmp) {
            Ok(_) => {
                assert!(
                    tmp.join("manifest.json").is_file(),
                    "staging reported success without a manifest, which is the \
                     failure that looks like success"
                );
                assert!(tmp
                    .join("_platform_specific")
                    .join(platform_dir())
                    .join(library_name())
                    .is_file());
            }
            Err(StageError::NoChrome(_)) | Err(StageError::NoCdm(_)) => {}
            Err(e) => panic!("unexpected error: {e}"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn staging_into_a_directory_with_no_chrome_does_not_panic() {
        let tmp = std::env::temp_dir().join(format!("fury-wv-test-{}", std::process::id()));
        // Whatever this machine has, the call must return a Result rather than
        // unwrap its way through a missing directory.
        let _ = stage(&tmp);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
