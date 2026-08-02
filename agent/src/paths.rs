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
pub fn data_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("FURY_HOME") {
        return PathBuf::from(explicit);
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());

    #[cfg(target_os = "macos")]
    let dir = PathBuf::from(home).join("Library/Application Support/Fury");

    #[cfg(target_os = "windows")]
    let dir = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(home))
        .join("Fury");

    #[cfg(all(unix, not(target_os = "macos")))]
    let dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&home).join(".local/share"))
        .join("fury");

    dir
}

pub fn db_path() -> PathBuf {
    data_dir().join("fury.db")
}

/// Where a profile's browser data lives between runs.
pub fn profile_dir(profile_id: &str) -> PathBuf {
    data_dir().join("profiles").join(profile_id)
}

/// The socket the desktop shell connects to.
///
/// A Unix socket rather than a TCP port, so nothing is listening on the network
/// and file permissions are the access control (docs/01).
///
/// Deliberately *not* under `data_dir()`. `sockaddr_un.sun_path` holds about
/// 104 bytes on macOS, and bind() fails with a message that never mentions
/// length — it says "path must be shorter than SUN_LEN", which sounds like a
/// programming error rather than "your home directory is deep". A long
/// username, an iCloud-synced home, or a FURY_HOME under a build directory all
/// reach the limit, so the socket lives somewhere short by construction.
///
/// The per-user temp directory is the right somewhere: on macOS `TMPDIR` is
/// already private to the user (`/var/folders/…/T/`), and on Linux
/// `XDG_RUNTIME_DIR` is `/run/user/<uid>`. Only when neither exists does this
/// fall back to a uid-suffixed name in the shared `/tmp`.
#[cfg(unix)]
pub fn socket_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("FURY_SOCKET") {
        return PathBuf::from(explicit);
    }

    // A per-installation suffix, so two data directories on one machine do not
    // fight over one socket — which is how a test run would otherwise talk to
    // the developer's real agent.
    let tag = short_tag(&data_dir());

    #[cfg(target_os = "macos")]
    let base = std::env::temp_dir();

    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());

    base.join(format!("fury-{tag}.sock"))
}

/// Eight hex characters derived from a path. Enough to separate installations,
/// short enough to stay inside the socket limit.
#[cfg(unix)]
fn short_tag(of: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(of.as_os_str().as_encoded_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// Creates the data directory if needed, readable only by this user.
///
/// The directory holds cookie jars for live accounts. World-readable would be a
/// gift to anything else running on the machine.
pub fn ensure_data_dir() -> std::io::Result<PathBuf> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_path_fits_in_a_sockaddr_un() {
        // 104 bytes on macOS, 108 on Linux; exceeding it fails at bind() with
        // "path must be shorter than SUN_LEN", which mentions neither the path
        // nor what to do about it.
        let len = socket_path().as_os_str().len();
        assert!(len < 100, "socket path is {len} bytes: {:?}", socket_path());
    }

    #[test]
    fn a_deep_data_directory_does_not_lengthen_the_socket_path() {
        // The regression: FURY_HOME under a long build path made the socket
        // path exceed the limit, and the daemon refused to start.
        let deep = std::path::Path::new("/private/tmp")
            .join("a".repeat(60))
            .join("b".repeat(60))
            .join("c".repeat(60));
        let tagged = short_tag(&deep);
        assert_eq!(tagged.len(), 8);
        // The tag is all the data directory contributes, so the socket path
        // cannot grow with it.
        assert!(socket_path().as_os_str().len() < 100);
    }

    #[test]
    fn two_installations_do_not_share_a_socket() {
        // Otherwise a test run with FURY_HOME set would connect to the agent
        // holding the developer's real profiles.
        assert_ne!(
            short_tag(std::path::Path::new("/home/a/Fury")),
            short_tag(std::path::Path::new("/home/b/Fury"))
        );
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
