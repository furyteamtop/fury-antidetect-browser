// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! How much disk a profile takes, and how much of it is throwaway.
//!
//! A profile grows to hundreds of megabytes and the operator has no way to see
//! it, no way to find which one is the big one, and no way to reclaim anything
//! short of deleting the profile and losing the account with it.
//!
//! The whole question is which bytes are safe to remove, and that question was
//! already answered once: `bundle::SKIP_CACHES` is the list of directories the
//! sync deliberately does not carry between machines, and it was chosen by
//! measuring three real profiles rather than by matching on the word "cache".
//! Two candidates were dropped from it for reasons that apply here word for
//! word — `Service Worker` holds registrations and dropping it can sign
//! somebody out; `Network Action Predictor` is typed-URL history, which is
//! behaviour rather than cache.
//!
//! So this module does not have a list. It uses that one.
//!
//! A second list would be the same decision made twice, and the second copy is
//! the one that quietly gains an entry nobody measured — which here means a
//! "clear cache" button that signs people out of their accounts.

use std::path::Path;

/// What a profile directory is made of.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Usage {
    /// Everything, in bytes.
    pub total: u64,
    /// The part `trim` would remove.
    pub cache: u64,
    /// Cookies, storage, history, sessions, preferences — the account.
    pub keep: u64,
    /// How many files were counted, so a wildly wrong number has a second
    /// figure beside it rather than standing alone.
    pub files: u64,
}

/// What `trim` removed.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Trimmed {
    pub bytes: u64,
    /// Named, not counted: an operator who has just deleted 300 MB is entitled
    /// to see what went.
    pub removed: Vec<String>,
}

/// Measures without touching anything.
pub fn measure(dir: &Path) -> std::io::Result<Usage> {
    let mut u = Usage::default();
    walk(dir, false, &mut u, &mut Vec::new())?;
    u.keep = u.total.saturating_sub(u.cache);
    Ok(u)
}

/// Removes the cache directories and reports what went.
///
/// Refuses while the browser is open, and the caller enforces that: Chromium
/// holds these open, and on Windows a directory with an open handle in it does
/// not delete — leaving a half-removed cache, which is worse than a full one.
pub fn trim(dir: &Path) -> std::io::Result<Trimmed> {
    let mut u = Usage::default();
    let mut removed = Vec::new();
    walk(dir, true, &mut u, &mut removed)?;
    Ok(Trimmed { bytes: u.cache, removed })
}

fn walk(
    dir: &Path,
    remove: bool,
    usage: &mut Usage,
    removed: &mut Vec<String>,
) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Symlinks are measured as themselves and never followed. Chromium
        // leaves Singleton* symlinks pointing at a socket, and following one
        // would leave this walking somewhere else entirely.
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            usage.total += meta.len();
            usage.files += 1;
            continue;
        }

        if meta.is_dir() && crate::bundle::is_cache_dir(&name) {
            let size = size_of_tree(&path)?;
            usage.total += size;
            usage.cache += size;
            if remove {
                std::fs::remove_dir_all(&path)?;
                removed.push(name);
            }
            continue;
        }

        if meta.is_dir() {
            walk(&path, remove, usage, removed)?;
        } else {
            usage.total += meta.len();
            usage.files += 1;
        }
    }
    Ok(())
}

fn size_of_tree(dir: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = std::fs::symlink_metadata(entry.path())?;
        total += if meta.is_dir() && !meta.file_type().is_symlink() {
            size_of_tree(&entry.path())?
        } else {
            meta.len()
        };
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("fury-usage-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A profile shaped like a real one: an account at the top level and a
    /// cache directory that dwarfs it.
    fn sample(root: &Path) {
        std::fs::create_dir_all(root.join("Default")).unwrap();
        std::fs::write(root.join("Default/Cookies"), vec![b'c'; 4096]).unwrap();
        std::fs::write(root.join("Default/History"), vec![b'h'; 2048]).unwrap();
        std::fs::write(root.join("Local State"), vec![b'l'; 512]).unwrap();

        std::fs::create_dir_all(root.join("Default/Cache")).unwrap();
        std::fs::write(root.join("Default/Cache/big"), vec![b'x'; 100_000]).unwrap();
        std::fs::create_dir_all(root.join("download_cache/sub")).unwrap();
        std::fs::write(root.join("download_cache/sub/blob"), vec![b'y'; 50_000]).unwrap();
    }

    #[test]
    fn cache_and_account_are_counted_apart() {
        let d = scratch("split");
        sample(&d);
        let u = measure(&d).unwrap();
        assert_eq!(u.cache, 150_000);
        assert_eq!(u.keep, 4096 + 2048 + 512);
        assert_eq!(u.total, u.cache + u.keep);
        assert_eq!(u.files, 3, "only the non-cache files are counted individually");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The claim the button makes, and the one that would lose somebody their
    /// account if it were false.
    #[test]
    fn trim_takes_the_cache_and_leaves_the_login() {
        let d = scratch("trim");
        sample(&d);

        let t = trim(&d).unwrap();
        assert_eq!(t.bytes, 150_000);
        assert!(t.removed.contains(&"Cache".to_string()), "{:?}", t.removed);
        assert!(t.removed.contains(&"download_cache".to_string()), "{:?}", t.removed);

        assert_eq!(std::fs::read(d.join("Default/Cookies")).unwrap().len(), 4096);
        assert_eq!(std::fs::read(d.join("Default/History")).unwrap().len(), 2048);
        assert!(d.join("Local State").exists());
        assert!(!d.join("Default/Cache").exists());
        assert!(!d.join("download_cache").exists());

        let after = measure(&d).unwrap();
        assert_eq!(after.cache, 0);
        assert_eq!(after.total, 4096 + 2048 + 512);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The two directories that were deliberately kept OUT of SKIP_CACHES.
    /// This is a test of the shared list, sitting here because this is the
    /// caller that would sign somebody out if the list ever grew.
    #[test]
    fn the_two_that_look_like_cache_and_are_not_survive() {
        let d = scratch("keepers");
        for dir in ["Service Worker", "Network Action Predictor"] {
            std::fs::create_dir_all(d.join("Default").join(dir)).unwrap();
            std::fs::write(d.join("Default").join(dir).join("f"), vec![b'z'; 1000]).unwrap();
        }
        let t = trim(&d).unwrap();
        assert_eq!(t.bytes, 0, "removed {:?}", t.removed);
        assert!(d.join("Default/Service Worker/f").exists());
        assert!(d.join("Default/Network Action Predictor/f").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_profile_that_was_never_launched_measures_zero_rather_than_failing() {
        let d = scratch("empty");
        let u = measure(&d).unwrap();
        assert_eq!(u.total, 0);
        assert_eq!(measure(&d.join("nope")).unwrap().total, 0);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Chromium leaves Singleton* as symlinks to a socket. Following one walks
    /// out of the profile; the earlier bundle work hit this and skips them.
    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_does_not_derail_the_walk() {
        let d = scratch("links");
        std::fs::write(d.join("real"), vec![b'r'; 100]).unwrap();
        std::os::unix::fs::symlink("/nowhere/at/all", d.join("SingletonLock")).unwrap();
        let u = measure(&d).unwrap();
        assert_eq!(u.files, 2);
        assert!(u.total >= 100);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
