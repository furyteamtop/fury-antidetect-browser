// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Taking a profile out of the Chrome the operator already uses.
//!
//! The cheapest way to acquire somebody from another product is to let them
//! bring their work. Donut Browser does this and it is the one feature on their
//! list with no technical depth to it — except for one thing, which has all of
//! the depth.
//!
//! ## Cookies do not travel, and saying otherwise would be the whole problem
//!
//! Chrome encrypts its cookie jar and its saved passwords with a key it keeps
//! in the operating system's keychain, under its own name. Copying `Cookies`
//! into a Fury profile produces a file the Fury core cannot read: it has its own
//! key (patch 0110), and Chrome's key belongs to Chrome.
//!
//! What Chromium does with a cookie jar it cannot decrypt is not complain — it
//! discards the rows. So the failure mode of pretending otherwise is an import
//! that reports success, opens, and is signed out of everything, with the
//! operator's own explanation being that Fury lost their accounts.
//!
//! Therefore this copies what travels and REFUSES to copy what does not, and
//! the refusal is in the result rather than in a log line. `cookies_skipped`
//! comes back as a number the caller has to show.
//!
//! Moving the cookies is a separate, deliberate act: Chrome's key can be read
//! from the keychain, but only by asking the operating system in front of the
//! operator, and the existing `profile.cookies.import` already accepts a jar
//! exported by whatever they trust to do that. One button that silently reaches
//! into another application's keychain is not a thing this should grow.
//!
//! ## What does travel
//!
//! History, bookmarks, preferences, autofill, local storage, IndexedDB, the
//! extension directories. Everything that makes a profile feel like the one
//! they had, minus the two files that are cryptographically bound elsewhere.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// A Chromium-family browser installed on this machine.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Browser {
    /// "chrome" | "edge" | "brave" | "chromium"
    pub kind: String,
    pub label: String,
    pub profiles: Vec<Found>,
}

/// One profile inside such a browser.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Found {
    /// The directory name — "Default", "Profile 1".
    pub dir: String,
    /// What the operator called it, from Local State. Falls back to `dir`.
    pub name: String,
    pub path: String,
}

/// What an import moved.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Imported {
    pub files: u64,
    pub bytes: u64,
    /// Files deliberately left behind because they are encrypted with the
    /// source browser's key. A number the caller must show, not a log line.
    pub cookies_skipped: u64,
}

/// Files bound to the SOURCE browser's OS-crypt key.
///
/// Copying one produces a file the core cannot decrypt, and Chromium's response
/// to an undecryptable jar is to drop the rows rather than to complain — so
/// carrying them would turn "your accounts are here" into "your accounts are
/// gone" with no error anywhere.
const ENCRYPTED_ELSEWHERE: &[&str] = &[
    "Cookies",
    "Cookies-journal",
    "Login Data",
    "Login Data-journal",
    "Login Data For Account",
    "Web Data",
    "Web Data-journal",
    "Affiliation Database",
];

/// Where each browser keeps its profiles, per platform.
fn roots() -> Vec<(&'static str, &'static str, PathBuf)> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    let home = PathBuf::from(home);

    #[cfg(target_os = "macos")]
    let base = home.join("Library/Application Support");
    #[cfg(windows)]
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home.join("AppData/Local"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = home.join(".config");

    #[cfg(target_os = "macos")]
    let paths = [
        ("chrome", "Google Chrome", base.join("Google/Chrome")),
        ("edge", "Microsoft Edge", base.join("Microsoft Edge")),
        ("brave", "Brave", base.join("BraveSoftware/Brave-Browser")),
        ("chromium", "Chromium", base.join("Chromium")),
    ];
    #[cfg(windows)]
    let paths = [
        ("chrome", "Google Chrome", base.join("Google/Chrome/User Data")),
        ("edge", "Microsoft Edge", base.join("Microsoft/Edge/User Data")),
        ("brave", "Brave", base.join("BraveSoftware/Brave-Browser/User Data")),
        ("chromium", "Chromium", base.join("Chromium/User Data")),
    ];
    #[cfg(all(unix, not(target_os = "macos")))]
    let paths = [
        ("chrome", "Google Chrome", base.join("google-chrome")),
        ("edge", "Microsoft Edge", base.join("microsoft-edge")),
        ("brave", "Brave", base.join("BraveSoftware/Brave-Browser")),
        ("chromium", "Chromium", base.join("chromium")),
    ];

    paths.into_iter().collect()
}

/// Every importable profile on this machine.
pub fn discover() -> Vec<Browser> {
    roots()
        .into_iter()
        .filter_map(|(kind, label, root)| {
            let profiles = profiles_in(&root);
            (!profiles.is_empty()).then(|| Browser {
                kind: kind.to_string(),
                label: label.to_string(),
                profiles,
            })
        })
        .collect()
}

fn profiles_in(root: &Path) -> Vec<Found> {
    if !root.is_dir() {
        return Vec::new();
    }
    // The display names live in Local State, keyed by directory name. Absent
    // or unreadable is not a failure — the directory name is a usable label.
    let names: serde_json::Value = std::fs::read(root.join("Local State"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(serde_json::Value::Null);

    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else { continue };
        // A profile is a directory with a Preferences file in it. Testing for
        // that rather than for the name catches "Profile 7" and misses
        // "ShaderCache", without a list of directories to keep right.
        if !dir.join("Preferences").is_file() {
            continue;
        }
        let label = names
            .pointer(&format!("/profile/info_cache/{name}/name"))
            .and_then(|v| v.as_str())
            .unwrap_or(name)
            .to_string();
        out.push(Found { dir: name.to_string(), name: label, path: dir.display().to_string() });
    }
    out
}

/// Copies `from` — a browser profile directory — into `into`.
///
/// `into` is a Fury profile's `Default`, because that is where the core will
/// look. Caches are left behind for the same reason the sync leaves them
/// (`bundle::is_cache_dir`), and the encrypted files are left behind for a
/// reason that matters more.
pub fn import(from: &Path, into: &Path) -> Result<Imported> {
    if !from.join("Preferences").is_file() {
        bail!(
            "{} does not look like a browser profile — no Preferences file",
            from.display()
        );
    }
    std::fs::create_dir_all(into)?;
    let mut report = Imported::default();
    copy_into(from, into, &mut report)?;
    Ok(report)
}

fn copy_into(from: &Path, into: &Path, report: &mut Imported) -> Result<()> {
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let src = entry.path();
        let meta = std::fs::symlink_metadata(&src)?;

        // Chromium leaves Singleton* pointing at a live socket; following one
        // walks out of the profile. bundle.rs learned this first.
        if meta.file_type().is_symlink() {
            continue;
        }

        if ENCRYPTED_ELSEWHERE.contains(&name.as_str()) {
            report.cookies_skipped += 1;
            continue;
        }

        if meta.is_dir() {
            if crate::bundle::is_cache_dir(&name) {
                continue;
            }
            let dst = into.join(&name);
            std::fs::create_dir_all(&dst)?;
            copy_into(&src, &dst, report)?;
        } else {
            std::fs::copy(&src, into.join(&name))
                .with_context(|| format!("copying {}", src.display()))?;
            report.files += 1;
            report.bytes += meta.len();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fury-imp-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A directory shaped like a real Chrome profile.
    fn chrome_profile(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("Preferences"), r#"{"profile":{"name":"x"}}"#).unwrap();
        std::fs::write(dir.join("History"), vec![b'h'; 500]).unwrap();
        std::fs::write(dir.join("Bookmarks"), vec![b'b'; 200]).unwrap();
        std::fs::write(dir.join("Cookies"), vec![b'c'; 900]).unwrap();
        std::fs::write(dir.join("Login Data"), vec![b'l'; 300]).unwrap();
        std::fs::create_dir_all(dir.join("Local Storage/leveldb")).unwrap();
        std::fs::write(dir.join("Local Storage/leveldb/000001.log"), vec![b's'; 100]).unwrap();
        std::fs::create_dir_all(dir.join("Cache")).unwrap();
        std::fs::write(dir.join("Cache/blob"), vec![b'x'; 10_000]).unwrap();
    }

    /// The claim the whole module rests on: what cannot be decrypted is left
    /// behind, and the number is reported rather than logged.
    #[test]
    fn the_encrypted_files_do_not_travel_and_the_count_comes_back() {
        let root = scratch("skip");
        let src = root.join("Default");
        chrome_profile(&src);
        let dst = root.join("out");

        let r = import(&src, &dst).unwrap();

        assert!(dst.join("History").exists());
        assert!(dst.join("Bookmarks").exists());
        assert!(dst.join("Local Storage/leveldb/000001.log").exists());

        assert!(!dst.join("Cookies").exists(), "a jar the core cannot read was copied");
        assert!(!dst.join("Login Data").exists());
        assert_eq!(r.cookies_skipped, 2);

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Caches are skipped using the one shared list, not a second one.
    #[test]
    fn caches_are_left_behind_by_the_same_rule_the_sync_uses() {
        let root = scratch("cache");
        let src = root.join("Default");
        chrome_profile(&src);
        let dst = root.join("out");

        let r = import(&src, &dst).unwrap();
        assert!(!dst.join("Cache").exists());
        assert_eq!(r.bytes, 500 + 200 + 100 + r#"{"profile":{"name":"x"}}"#.len() as u64);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn something_that_is_not_a_profile_is_named_as_such() {
        let root = scratch("notprofile");
        std::fs::write(root.join("random.txt"), "x").unwrap();
        let err = import(&root, &root.join("out")).unwrap_err().to_string();
        assert!(err.contains("Preferences"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Discovery keys off Preferences rather than off a list of directory
    /// names, so "Profile 7" is found and "ShaderCache" is not.
    #[test]
    fn profiles_are_found_by_shape_and_named_from_local_state() {
        let root = scratch("discover");
        chrome_profile(&root.join("Default"));
        chrome_profile(&root.join("Profile 7"));
        std::fs::create_dir_all(root.join("ShaderCache")).unwrap();
        std::fs::write(
            root.join("Local State"),
            r#"{"profile":{"info_cache":{"Profile 7":{"name":"Work"}}}}"#,
        )
        .unwrap();

        let found = profiles_in(&root);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].dir, "Default");
        // No entry in Local State: the directory name is the label.
        assert_eq!(found[0].name, "Default");
        assert_eq!(found[1].dir, "Profile 7");
        assert_eq!(found[1].name, "Work");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_browser_that_is_not_installed_is_absent_rather_than_empty() {
        assert!(profiles_in(Path::new("/nowhere/at/all")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_singleton_symlink_is_not_followed() {
        let root = scratch("link");
        let src = root.join("Default");
        chrome_profile(&src);
        std::os::unix::fs::symlink("/nowhere", src.join("SingletonLock")).unwrap();
        let dst = root.join("out");
        import(&src, &dst).unwrap();
        assert!(!dst.join("SingletonLock").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
