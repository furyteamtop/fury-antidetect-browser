// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Where the agent keeps things.
//!
//! One directory holds the local database, the profile data dirs and the IPC
//! socket, so "back up Fury" and "wipe Fury" are each one operation on one path.

use std::path::PathBuf;

/// Root of everything this machine's agent owns.
///
/// Deliberately not the desktop shell's config directory: the agent runs with
/// the UI closed (docs/01 — it holds locks and serves the automation API), and
/// tying its data to a GUI application's identity would be wrong the first time
/// someone runs the agent headless on a box with no desktop.
pub use fury_platform::dirs::data_dir;

/// Personas added on this machine, beside the built-in catalogue: a capture of
/// this computer, kept here instead of published. See personas.rs.
pub fn personas_dir() -> PathBuf {
    data_dir().join("personas")
}

pub fn db_path() -> PathBuf {
    data_dir().join("fury.db")
}

/// Where an installed core lives.
///
/// Separate from the desktop shell's application bundle, and that separation is
/// the point. The shell is 12 MB and the core is 134 MB compressed; they are
/// versioned apart, and putting the core inside the shell's bundle would break
/// the shell's code signature every time the core was replaced — a signed
/// application whose contents changed underneath it is one macOS refuses to
/// launch, with a message about damage rather than about signing.
///
/// So the shell is a signed bundle that stays as it was shipped, and the core
/// is data next to the profiles. `fury-agent install-core` puts it here.
///
/// The `.bundle` extension is not decoration. Spotlight indexes a top-level
/// `.app` as an application, so an installed core showed up in search as a
/// second thing called Fury, beside the one the user actually opens. It does
/// NOT index inside a package, which is why real Chrome shows one result and
/// not six despite carrying five helper applications.
///
/// Measured rather than assumed, because the obvious fix does not work: a
/// `.metadata_never_index` file is the documented way to ask, and on macOS 26
/// two identical bundles — one with the marker, one without — were both
/// indexed. A package extension was tried the same way and hid its contents.
pub fn core_dir() -> PathBuf {
    data_dir().join("core.bundle")
}

/// Which release the installed core came from, written by install_core.
///
/// Beside core.bundle rather than in it: an install replaces that directory
/// whole, and the record must describe whatever is there after it.
pub fn core_release_file() -> PathBuf {
    data_dir().join("core.release")
}

/// Where a core installed by an earlier version sits.
///
/// Renamed rather than left: two copies of a 544 MB browser is not a tidy way
/// to change a directory name, and a core the agent no longer looks at is a
/// browser that silently stops being used.
pub fn legacy_core_dir() -> PathBuf {
    data_dir().join("core")
}

/// Where each profile's copy of the core, named after the profile, is made
/// for the macOS Dock (dock.rs).
///
/// `.noindex`, not `.bundle`: measured 12.10.2026, Spotlight lists
/// core.bundle/Fury.app as an application despite what core_dir says, and
/// lists nothing under a `.noindex` directory (core/src/out/*.noindex holds
/// several Fury.app and none appears). A clone per profile in search would be
/// a list of fake applications named after accounts.
pub fn dock_dir() -> PathBuf {
    data_dir().join("dock.noindex")
}

/// Where a profile's browser data lives between runs.
pub fn profile_dir(profile_id: &str) -> PathBuf {
    data_dir().join("profiles").join(profile_id)
}

/// Where named blocklists are kept.
///
/// Files rather than database rows. A published list is a hundred thousand
/// domains; copying that into every profile that uses it would be the same
/// megabytes stored fifty times, and the profile row would stop being something
/// a person can read.
pub fn blocklists_dir() -> PathBuf {
    data_dir().join("blocklists")
}

/// Saved profile templates (docs/16 5.6): one JSON file, a few kilobytes. A
/// template is a named set of answers to the batch dialog — proxy, tags,
/// stage, sites, languages — not a profile, and not a table.
pub fn templates_file() -> PathBuf {
    data_dir().join("templates.json")
}

/// Farming plans and where each profile is in them (farm.rs): one JSON file,
/// rewritten when it changes.
pub fn farms_file() -> PathBuf {
    data_dir().join("farms.json")
}

/// Where this profile's unpacked extensions sit.
///
/// INSIDE the profile directory, which means the team bundle carries them
/// without anything extra being written — and it means every profile has its
/// own copy at its own path. That would normally give the same extension a
/// different id in every profile; `ext::install` writes the developer key into
/// the manifest so it does not. See `agent/src/ext.rs`.
///
/// The space in the name is deliberate: Chromium's own directories under a
/// profile are `Local Extension Settings`, `Extension Rules`, `Extension
/// State`. A bare `Extensions` would collide with the one Chromium creates for
/// packed extensions it manages itself.
pub fn extensions_dir(profile_id: &str) -> PathBuf {
    profile_dir(profile_id).join(EXTENSIONS_DIR)
}

/// The name of [`extensions_dir`] inside a profile directory.
pub const EXTENSIONS_DIR: &str = "Fury Extensions";

/// Where the desktop shell reaches this agent.
///
/// A Unix socket on macOS, a named pipe on Windows. Both this and the shell
/// call the same function in `fury_platform::dirs`, which is the point — the
/// two find each other by computing the same address from the same inputs, and
/// there is no discovery step to fall back on if they disagree.
pub use fury_platform::dirs::ipc_endpoint;

/// Creates the data directory if needed, readable only by this user.
///
/// The directory holds cookie jars for live accounts. World-readable would be a
/// gift to anything else running on the machine.
pub fn ensure_data_dir() -> std::io::Result<PathBuf> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    fury_platform::perms::owner_only_dir(&dir)?;
    Ok(dir)
}

/// Where the agent writes its log, once [`crate::logging`] has made it.
///
/// Inside the data directory rather than beside the application: the agent is
/// a separate process that outlives the window, and its log belongs with the
/// profiles it is holding, in a directory that is already owner-only.
pub fn log_file() -> PathBuf {
    data_dir().join("logs").join("agent.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    // The address itself is tested in fury_platform::dirs, which is now the
    // only place that computes it.

    #[test]
    fn the_core_lives_in_a_package_so_search_shows_one_fury() {
        // The whole reason for the extension. Losing it silently would put a
        // second "Fury" in everybody's Spotlight and nobody would connect the
        // two.
        assert_eq!(
            core_dir().extension().and_then(|e| e.to_str()),
            Some("bundle"),
            "core_dir must keep a package extension — see the note on it"
        );
    }

    #[test]
    fn an_installed_core_is_under_the_same_root() {
        // "Wipe Fury" has to remove the browser as well, or 134 MB survives an
        // uninstall in a directory nobody would think to look in.
        assert!(core_dir().starts_with(data_dir()));
    }

    #[test]
    fn the_data_a_backup_needs_lives_under_one_root() {
        let root = data_dir();
        assert!(db_path().starts_with(&root));
        assert!(profile_dir("abc").starts_with(&root));
        // The socket is explicitly not here: it is a runtime object, and
        // copying one into a backup would be meaningless.
    }
}
