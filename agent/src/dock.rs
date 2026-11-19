// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The profile's name under its icon in the macOS Dock.
//!
//! Every profile is the same signed core, so every one of them said "Fury"
//! in the Dock, and with five open the only way to find an account was to
//! click through them (tester, 12.10.2026).
//!
//! What the Dock shows is the application bundle's DIRECTORY name, not
//! CFBundleName and not anything the running process says about itself.
//! Measured on a two-line Cocoa program: CFBundleName "Fury" in a bundle named
//! DockName.app showed "DockName"; the same bundle copied to "console1 TR.app"
//! showed "console1 TR"; renaming the running process through LaunchServices
//! (_LSSetApplicationInformationItem, kLSDisplayNameKey) changed what lsappinfo
//! reported and left the Dock as it was.
//!
//! So each launch runs a copy of the core named after the profile. A clone,
//! not a copy (clonefile: 0.12 s, no blocks written), in dock.noindex so that
//! Spotlight does not list it as an application (paths::dock_dir). What was
//! checked on the notarised core before relying on it:
//!
//!   - the signature survives: a bundle's directory name is not part of what
//!     is signed (`codesign --verify --deep --strict`, and `spctl` still says
//!     "Notarized Developer ID");
//!   - the keychain does not ask: the Safe Storage entry trusts the core by
//!     its designated requirement (identifier dev.fury.core, team JKN8WHX7FS),
//!     not by path, so a clone anywhere opens it like the original;
//!   - the menu bar still says Fury: that one is CFBundleName.
//!
//! Remade on every launch -- the core may have been updated since the last
//! one, and a profile is never launched twice at once, so its clone is never
//! replaced under a running browser. Any failure here is not a failed launch:
//! the original core runs, and the Dock says Fury, as before.
//!
//! And gone once the browser is (tidy). A clone carries the core's Info.plist
//! whole, document types and http/https/file included, so LaunchServices took
//! every clone it had seen for a browser, and Finder's "Open With" listed one
//! "Fury" per profile -- 109 of them for a tester, 20.11.2026, along with the
//! clones of profiles renamed since. Taking the types out of the clone's
//! Info.plist is not open to us: the Info.plist is sealed by the signature
//! (codesign: "invalid Info.plist (plist or signature have been modified)",
//! measured on a clone), and a clone that fails its signature is a core the
//! keychain no longer trusts. Deleting the bundle is not enough on its own
//! either: LaunchServices kept records of clones deleted days before (lsregister
//! -dump), so each one is unregistered first, while it is still there to scan.

use std::path::{Path, PathBuf};

/// Held from the clone until the browser runs from it, and by tidy for its
/// whole pass, so that tidy never deletes a clone that a launch has made and
/// not yet started (no process runs from it yet, and it looks unused).
#[cfg(target_os = "macos")]
static CLONES: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_os = "macos")]
fn hold() -> std::sync::MutexGuard<'static, ()> {
    // Nothing behind the lock can be left half-changed by a panic that tidy
    // or a launch would trip over: it guards the directory, not data.
    CLONES.lock().unwrap_or_else(|e| e.into_inner())
}

/// A profile's named copy of the core. Keep it until the browser has been
/// spawned from `exe`: until then, tidy is held off.
#[cfg(target_os = "macos")]
pub struct NamedCore {
    pub exe: PathBuf,
    _held: std::sync::MutexGuard<'static, ()>,
}

/// The name as the Dock will show it, or None to leave it "Fury".
///
/// A directory name, so the two characters a macOS path cannot hold in a
/// component -- `/`, and `:` which Finder shows as `/` -- become `-`, control
/// characters go, a leading dot (a hidden directory) goes, and it is kept to
/// a length the Dock can show anyway.
pub fn display_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '/' || c == ':' { '-' } else { c })
        .take(80)
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The executable of this profile's named copy of the core, made now.
///
/// `core_binary` is `<X>.app/Contents/MacOS/<exe>`; the copy is
/// `dock.noindex/<profile id>/<name>.app` with the same path inside.
#[cfg(target_os = "macos")]
pub fn named_core(core_binary: &Path, profile_id: &str, name: &str) -> std::io::Result<NamedCore> {
    let held = hold();
    let exe = named_core_in(&crate::paths::dock_dir(), core_binary, profile_id, name)?;
    Ok(NamedCore { exe, _held: held })
}

/// Unregister and delete every clone no process runs from.
///
/// Run when a browser closes, and once when the agent starts, which also takes
/// care of what earlier versions left: a clone per profile ever opened, a
/// renamed profile's clone under its old name, and the clones of deleted
/// profiles. A clone some process still runs from stays, whoever started it --
/// a browser that outlived its agent included: deleting its bundle would leave
/// it unable to start a renderer. Best effort and quiet, like the clone.
#[cfg(target_os = "macos")]
pub fn tidy() {
    let _held = hold();
    let Some(running) = fury_platform::process::executables() else {
        tracing::warn!("could not list processes; the profiles' Dock copies stay for now");
        return;
    };
    let removed = tidy_in(&crate::paths::dock_dir(), &running, unregister);
    if removed > 0 {
        tracing::info!(removed, "Dock copies of closed profiles removed");
    }
}

/// Every `<profile id>/` under `dock_dir` that no path in `running` is inside:
/// its bundles handed to `unregister`, then the directory deleted. Returns how
/// many directories went.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn tidy_in(dock_dir: &Path, running: &[PathBuf], unregister: impl Fn(&[PathBuf])) -> usize {
    // The kernel reports resolved paths; FURY_HOME under /tmp is /private/tmp.
    let Ok(root) = dock_dir.canonicalize() else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() || running.iter().any(|p| p.starts_with(&dir)) {
            continue;
        }
        let bundles: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|it| {
                it.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "app"))
                    .collect()
            })
            .unwrap_or_default();
        if !bundles.is_empty() {
            unregister(&bundles);
        }
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(dir = %dir.display(), error = %e, "could not remove a Dock copy"),
        }
    }
    removed
}

/// Forget these bundles in LaunchServices, and the helper apps inside them.
///
/// lsregister rather than an API: there is no public call that unregisters,
/// only LSRegisterURL. One process for all of them. Measured: `-u` takes a
/// bundle's record out of `lsregister -dump`, in 6 ms. The helpers (Renderer,
/// GPU, Alerts...) are registered on their own and declare no document types,
/// so they were never in Open With, but their records would stay forever;
/// named here one by one, since `-R` scans the whole 300 MB clone and took
/// 1.4 s a clone, minutes for a hundred with launches waiting on the lock.
#[cfg(target_os = "macos")]
fn unregister(bundles: &[PathBuf]) {
    const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
                              LaunchServices.framework/Support/lsregister";
    // <X>.app/Contents/Frameworks/<F>.framework/Versions/<v>/Helpers/<H>.app
    let list = |p: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(p).map(|it| it.flatten().map(|e| e.path()).collect()).unwrap_or_default()
    };
    let mut all = bundles.to_vec();
    for app in bundles {
        for framework in list(&app.join("Contents/Frameworks")) {
            for version in list(&framework.join("Versions")) {
                all.extend(
                    list(&version.join("Helpers")).into_iter().filter(|h| h.extension().is_some_and(|e| e == "app")),
                );
            }
        }
    }
    let out = std::process::Command::new(LSREGISTER)
        .arg("-u")
        .args(&all)
        .stdin(std::process::Stdio::null())
        .output();
    // The status says little: lsregister exits 1 for a path it has no record
    // of (-10814), which most helpers are, so only failing to run it is news.
    match out {
        Ok(o) => tracing::debug!(status = %o.status, paths = all.len(), "lsregister -u"),
        Err(e) => tracing::warn!(error = %e, "could not run lsregister; Open With may still list them"),
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn named_core_in(dock_dir: &Path, core_binary: &Path, profile_id: &str, name: &str) -> std::io::Result<PathBuf> {
    let invalid = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, what.to_string());
    let name = display_name(name).ok_or_else(|| invalid("the profile has no name to show"))?;
    // A directory component, so it has to be one.
    if profile_id.is_empty() || !profile_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(invalid("not a profile id"));
    }
    let exe = core_binary.file_name().ok_or_else(|| invalid("no executable"))?;
    let app = core_binary
        .parent()
        .filter(|p| p.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or_else(|| invalid("the core is not inside an application bundle"))?;

    let dir = dock_dir.join(profile_id);
    // Whatever the last launch left, including a clone under another name.
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let named = dir.join(format!("{name}.app"));
    clone(app, &named)?;
    Ok(named.join("Contents/MacOS").join(exe))
}

#[cfg(target_os = "macos")]
fn clone(src: &Path, dst: &Path) -> std::io::Result<()> {
    fury_platform::tree::clone_tree(src, dst)
}

#[cfg(not(target_os = "macos"))]
fn clone(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("the Dock is a macOS thing"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_one_directory_component() {
        assert_eq!(display_name("console1.access@gmail.com | TR").as_deref(), Some("console1.access@gmail.com | TR"));
        assert_eq!(display_name("a/b:c").as_deref(), Some("a-b-c"));
        assert_eq!(display_name("  .hidden ").as_deref(), Some("hidden"));
        assert_eq!(display_name("tab\there\n").as_deref(), Some("tabhere"));
        assert_eq!(display_name("   "), None);
        assert_eq!(display_name("..."), None);
        assert_eq!(display_name(&"x".repeat(300)).map(|s| s.chars().count()), Some(80));
        assert_eq!(display_name("Профиль 1").as_deref(), Some("Профиль 1"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_copy_is_named_after_the_profile_and_remade_each_time() {
        let root = std::env::temp_dir().join(format!("fury-dock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let core = root.join("core.bundle/Fury.app/Contents/MacOS");
        std::fs::create_dir_all(&core).unwrap();
        std::fs::write(core.join("Fury"), b"v1").unwrap();
        let dock = root.join("dock.noindex");

        let exe = named_core_in(&dock, &core.join("Fury"), "p-1", "kg-seo 1").unwrap();
        assert_eq!(exe, dock.join("p-1/kg-seo 1.app/Contents/MacOS/Fury"));
        assert_eq!(std::fs::read(&exe).unwrap(), b"v1");

        // A new core and a renamed profile: the old clone goes, the new one
        // carries the new core.
        std::fs::write(core.join("Fury"), b"v2").unwrap();
        let exe = named_core_in(&dock, &core.join("Fury"), "p-1", "kg-seo 2").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"v2");
        assert!(!dock.join("p-1/kg-seo 1.app").exists());

        assert!(named_core_in(&dock, &core.join("Fury"), "../x", "n").is_err());
        assert!(named_core_in(&dock, Path::new("/usr/bin/true"), "p-2", "n").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tidy_takes_only_the_copies_nothing_runs_from() {
        let root = std::env::temp_dir().join(format!("fury-dock-tidy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dock = root.join("dock.noindex");
        for (id, name) in [("open", "A"), ("closed", "B"), ("renamed", "Old"), ("renamed", "New")] {
            std::fs::create_dir_all(dock.join(id).join(format!("{name}.app/Contents/MacOS"))).unwrap();
        }
        // As the kernel reports it: resolved.
        let real = dock.canonicalize().unwrap();
        let running = vec![real.join("open/A.app/Contents/MacOS/Fury"), PathBuf::from("/usr/bin/true")];

        let seen = std::cell::RefCell::new(Vec::new());
        let removed = tidy_in(&dock, &running, |b| seen.borrow_mut().extend(b.iter().cloned()));
        assert_eq!(removed, 2);
        assert!(dock.join("open/A.app").exists());
        assert!(!dock.join("closed").exists());
        assert!(!dock.join("renamed").exists());
        let mut seen: Vec<String> =
            seen.into_inner().iter().map(|p| p.strip_prefix(&real).unwrap().display().to_string()).collect();
        seen.sort();
        assert_eq!(seen, ["closed/B.app", "renamed/New.app", "renamed/Old.app"]);

        // Nothing there at all is not an error.
        assert_eq!(tidy_in(&root.join("missing"), &[], |_| panic!("nothing to unregister")), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
