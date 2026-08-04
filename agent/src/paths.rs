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

/// Where a core installed by an earlier version sits.
///
/// Renamed rather than left: two copies of a 544 MB browser is not a tidy way
/// to change a directory name, and a core the agent no longer looks at is a
/// browser that silently stops being used.
pub fn legacy_core_dir() -> PathBuf {
    data_dir().join("core")
}

/// Where a profile's browser data lives between runs.
pub fn profile_dir(profile_id: &str) -> PathBuf {
    data_dir().join("profiles").join(profile_id)
}

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
