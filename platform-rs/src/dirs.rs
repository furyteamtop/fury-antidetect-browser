// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Where Fury keeps things, and the tag that follows from it.
//!
//! This is here rather than in the agent for one reason: two programs have to
//! agree on it exactly. The agent listens at an address derived from the data
//! directory, and the desktop shell connects to an address it derives the same
//! way. There is no discovery step. If the two computations ever differ by a
//! character, the shell reports "the agent is not running" forever, against an
//! agent that is running, and nothing in either log says why.
//!
//! It WAS in two places — `agent/src/paths.rs` and
//! `desktop/src-tauri/src/agent.rs`, with a comment in each saying it had to
//! match the other. They had already drifted: the shell's copy had a macOS
//! branch and a non-macOS-Unix branch and no Windows branch at all, so the
//! first Windows build of the shell would not have compiled — and if the
//! missing branch had been filled in by hand with `LOCALAPPDATA` instead of the
//! agent's `APPDATA`, it would have compiled and then never found the agent.
//!
//! A comment asking two files to stay in step is not a mechanism. One function
//! is.

use std::path::{Path, PathBuf};

/// Root of everything this machine's Fury owns.
///
/// `FURY_HOME` overrides it, which is how tests and a second installation get
/// their own.
///
/// Deliberately not the desktop shell's config directory: the agent runs with
/// the UI closed — it holds locks and serves the automation API (docs/01) — and
/// tying its data to a GUI application's identity would be wrong the first time
/// someone runs the agent headless.
pub fn data_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("FURY_HOME") {
        if !explicit.is_empty() {
            return PathBuf::from(explicit);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join("Library/Application Support/Fury")
    }

    // Not a shipped target — see lib.rs — but the Unix path compiles here so
    // that CI and anyone working on this from a Linux machine can run the test
    // suite. Refusing to compile would be a different decision from refusing to
    // release, and only the second one was made.
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(&home).join(".local/share"))
            .join("fury")
    }

    #[cfg(windows)]
    {
        // APPDATA, not LOCALAPPDATA. The difference is roaming profiles: on a
        // domain-joined machine APPDATA follows the user between machines and
        // LOCALAPPDATA does not. Fury's directory holds profile data that is
        // hundreds of megabytes, which is an argument for LOCALAPPDATA — and
        // the argument the other way wins, because the agent's local database
        // records which profiles this user has and losing it on a different
        // machine looks like data loss rather than like a cache miss.
        //
        // Whichever is right, both programs have to pick the SAME one, which is
        // the reason this function exists at all.
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()))
                    .join("AppData/Roaming")
            })
            .join("Fury")
    }
}

/// Eight hex characters derived from a path.
///
/// Enough to separate two installations on one machine — so a test run with
/// `FURY_HOME` set does not connect to the agent holding the developer's real
/// profiles, and then launch browsers in them — and short enough to keep a Unix
/// socket path inside `sun_path`.
///
/// Not a security boundary. It is derived from a path that is not secret, and
/// on Windows the address it ends up in is a pipe name any process can read.
/// What keeps other users out is the DACL, not the name.
pub fn short_tag(of: &Path) -> String {
    // SHA-256, truncated. `DefaultHasher` would be the obvious alternative and
    // is explicitly not stable between Rust releases — which would move the
    // address under a running agent on a toolchain upgrade, and the two
    // programs would then disagree depending on which compiler built each.
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(of.as_os_str().as_encoded_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// The address the agent listens on and the shell connects to.
pub fn ipc_endpoint() -> crate::Endpoint {
    crate::ipc::endpoint(&short_tag(&data_dir()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_installations_do_not_share_an_address() {
        assert_ne!(
            short_tag(Path::new("/home/a/Fury")),
            short_tag(Path::new("/home/b/Fury"))
        );
    }

    #[test]
    fn the_tag_is_eight_characters_whatever_the_path() {
        let deep = Path::new("/private/tmp")
            .join("a".repeat(60))
            .join("b".repeat(60))
            .join("c".repeat(60));
        assert_eq!(short_tag(&deep).len(), 8);
        // The regression this was written for: FURY_HOME under a long build
        // path made the socket path exceed sun_path, and the agent refused to
        // start with a message about SUN_LEN that mentioned neither the path
        // nor what to do about it.
        assert!(ipc_endpoint().to_string().len() < 100);
    }

    #[test]
    fn the_data_directory_is_under_the_users_own_profile() {
        // Not a strong assertion, deliberately — the point is that it is
        // somewhere per-user rather than a fixed path two accounts would share.
        let dir = data_dir().to_string_lossy().to_string();
        assert!(!dir.is_empty());
        assert!(!dir.starts_with("/tmp/"), "{dir}");
    }
}
