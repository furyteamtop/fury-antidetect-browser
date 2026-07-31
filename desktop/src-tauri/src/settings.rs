//! Where this installation points, and what it calls itself.
//!
//! Fury is self-hosted, so there is no address to hard-code: the first thing a
//! new installation needs is somewhere to connect to. That makes the server URL
//! a first-class setting rather than a build-time constant, and the app has to
//! be usable before one exists.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Base URL of the Fury server, without a trailing slash. `None` until the
    /// operator has been asked.
    pub server_url: Option<String>,

    /// Stable identifier for this installation.
    ///
    /// The server uses it to recognise a returning machine, which is what lets
    /// an agent reclaim a lock it already holds after a crash instead of
    /// waiting out the timeout.
    pub machine_id: String,
}

impl Settings {
    /// Loads settings, or produces fresh ones with a new machine id.
    ///
    /// A corrupt file is replaced rather than fatal: the only thing that would
    /// be lost is the machine id, and refusing to start over a malformed
    /// preferences file would strand the operator with no way back in.
    pub fn load(dir: &PathBuf) -> Self {
        let path = Self::path(dir);
        match std::fs::read_to_string(&path).ok().and_then(|raw| {
            serde_json::from_str::<Settings>(&raw)
                .inspect_err(|e| eprintln!("settings: ignoring unreadable {path:?}: {e}"))
                .ok()
        }) {
            Some(mut s) => {
                if s.machine_id.is_empty() {
                    s.machine_id = new_machine_id();
                }
                s
            }
            None => Settings {
                server_url: None,
                machine_id: new_machine_id(),
            },
        }
    }

    pub fn save(&self, dir: &PathBuf) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self)?;
        // Write-then-rename: a power cut mid-write would otherwise leave a
        // truncated file, and the recovery above would silently mint a new
        // machine id — which the server would read as a different computer.
        let tmp = Self::path(dir).with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(tmp, Self::path(dir))?;
        Ok(())
    }

    fn path(dir: &PathBuf) -> PathBuf {
        dir.join("settings.json")
    }
}

fn new_machine_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// What this machine is called in the profile lock column.
///
/// The browser build could only offer `navigator.platform`, so every Mac in an
/// organisation showed up as "MacIntel" — useless for the one question the
/// column answers, which is *whose* machine is holding the profile. Here the
/// real device name is available.
pub fn machine_name() -> String {
    let name = whoami::devicename();
    if name.trim().is_empty() {
        "unknown machine".to_string()
    } else {
        name
    }
}

/// Normalises a server URL as typed by a human.
///
/// Accepts `fury.example.com`, `https://fury.example.com/`, and
/// `http://127.0.0.1:8901`. Rejects anything that is not http(s), because a
/// typo'd scheme would otherwise surface much later as an opaque transport
/// error.
pub fn normalise_server_url(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Enter the address of your Fury server.".into());
    }

    // Split the scheme off *before* touching trailing slashes. Stripping them
    // first turns "https://" into "https:", which no longer looks like it has a
    // scheme and so gets another one prepended — "https://https:", an address
    // that would be saved happily and fail much later.
    let (scheme, rest) = match trimmed.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r),
        // Default to https. Someone self-hosting on plain http has to say so,
        // rather than having credentials sent in the clear because they left
        // the scheme off.
        None => ("https".to_string(), trimmed),
    };

    if scheme != "http" && scheme != "https" {
        return Err("The address must start with http:// or https://".into());
    }

    let host = rest.trim_end_matches('/');
    if host.is_empty() {
        return Err("That address has no host.".into());
    }
    Ok(format!("{scheme}://{host}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_host_becomes_https() {
        assert_eq!(
            normalise_server_url("fury.example.com").unwrap(),
            "https://fury.example.com"
        );
    }

    #[test]
    fn plain_http_must_be_spelled_out() {
        // Not rejected — self-hosting on a LAN is legitimate — but it cannot
        // happen by omission.
        assert_eq!(
            normalise_server_url("http://127.0.0.1:8901").unwrap(),
            "http://127.0.0.1:8901"
        );
    }

    #[test]
    fn trailing_slashes_do_not_produce_double_slashes_later() {
        assert_eq!(
            normalise_server_url("  https://fury.example.com/  ").unwrap(),
            "https://fury.example.com"
        );
    }

    #[test]
    fn other_schemes_are_refused() {
        assert!(normalise_server_url("ftp://fury.example.com").is_err());
        assert!(normalise_server_url("file:///etc/passwd").is_err());
        assert!(normalise_server_url("").is_err());
        assert!(normalise_server_url("   ").is_err());
    }

    #[test]
    fn a_scheme_with_no_host_is_refused() {
        // Regression: stripping trailing slashes before splitting the scheme
        // turned this into "https://https:" and accepted it.
        assert!(normalise_server_url("https://").is_err());
        assert!(normalise_server_url("http://").is_err());
        assert!(normalise_server_url("https:///").is_err());
    }

    #[test]
    fn the_scheme_is_normalised_but_the_host_is_not() {
        // Hostnames are case-insensitive in DNS but a path would not be, and
        // the server URL is used as the keychain account name — changing its
        // case would orphan a stored token.
        assert_eq!(
            normalise_server_url("HTTPS://Fury.Example.com").unwrap(),
            "https://Fury.Example.com"
        );
    }

    #[test]
    fn a_machine_always_has_a_name() {
        assert!(!machine_name().trim().is_empty());
    }
}
