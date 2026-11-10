// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Where this installation points, and what it calls itself.
//!
//! Fury is self-hosted, so there is no address to hard-code: the first thing a
//! new installation needs is somewhere to connect to. That makes the server URL
//! a first-class setting rather than a build-time constant, and the app has to
//! be usable before one exists.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// See `Settings::own_public_key`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnPublicKey {
    pub server: String,
    pub email: String,
    /// base64, as sent to the server at enrolment.
    pub key: String,
}

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

    /// The address that signed in last, so unlocking after a restart asks for a
    /// password rather than for both. Not a credential — the token is in the
    /// keychain and the key it protects is not stored anywhere.
    #[serde(default)]
    pub last_email: Option<String>,

    /// The last server this installation was pointed at, kept after it stops
    /// being pointed at one.
    ///
    /// `server_url` is the live connection and goes to None the moment somebody
    /// chooses to work locally. This one does not, and that is the whole point:
    /// a session lasts twelve hours, so a machine left overnight comes back to
    /// a sign-in screen, and one press of "work without an account" underneath
    /// it forgot the address as well. What was left was the first-run screen,
    /// asking for a server, from somebody who had one — with the token and the
    /// organisation key still sitting in the keychain, unreachable because
    /// nothing remembered which server they belonged to.
    ///
    /// Not a credential: an address, beside the address that signed in.
    #[serde(default)]
    pub last_server: Option<String>,

    /// This installation's own public key, as generated or opened here, with
    /// the server and address it belongs to: what the verification code on the
    /// "waiting to be let in" screen is computed from (crypto::verify_code).
    /// Kept here rather than asked of the server, because the point of the
    /// code is to check what the server lists. Not a secret.
    #[serde(default)]
    pub own_public_key: Option<OwnPublicKey>,

    /// Where the one number below lived before it was kept per server. Read
    /// from an old file, moved into `ork_generations` by `load`, written as 0.
    #[serde(default)]
    pub ork_generation: i32,

    /// The highest organisation-key generation this machine has accepted from
    /// each server, by address. Only ever moves forward, except when this
    /// machine itself starts over on a server (`start_generation`).
    ///
    /// The wrapping of an organisation key carries no signature, so a server
    /// that kept a member's pre-rotation blob could serve it back and return
    /// them to a key a removed colleague still holds. A watermark that only
    /// moves forward makes that a refusal.
    ///
    /// Per server because generations are counted per organisation. Until
    /// 0.5.2 it was one number for the machine: somebody who had rotated a key
    /// on one server and then set up a second one could create its owner and
    /// never sign in again, the new organisation being at generation 1 and the
    /// machine remembering 2. The sign-in screen called that a wrong password.
    #[serde(default)]
    pub ork_generations: std::collections::BTreeMap<String, i32>,

    /// Whether the organisation key is remembered between launches.
    ///
    /// On by default. Off means the password is asked for on every start,
    /// which is a real choice for a shared machine and a bad default for a
    /// tool somebody opens ten times a day.
    #[serde(default = "yes")]
    pub remember_org_key: bool,
}

fn yes() -> bool {
    true
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
                s.adopt_legacy_generation();
                s
            }
            None => Settings {
                last_email: None,
                last_server: None,
                own_public_key: None,
                ork_generation: 0,
                ork_generations: Default::default(),
                remember_org_key: true,
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

    /// The single number an older version kept belongs to the server that
    /// version was last pointed at: the only one it can have come from that
    /// this file still names.
    fn adopt_legacy_generation(&mut self) {
        if self.ork_generation == 0 {
            return;
        }
        if let Some(server) = self.server_url.clone().or_else(|| self.last_server.clone()) {
            let seen = self.ork_generations.entry(server).or_insert(0);
            *seen = (*seen).max(self.ork_generation);
        }
        self.ork_generation = 0;
    }

    /// The highest generation accepted from the server this installation is
    /// pointed at. 0 when it is pointed at none, or has accepted none.
    pub fn seen_generation(&self) -> i32 {
        self.server_url
            .as_deref()
            .and_then(|s| self.ork_generations.get(s))
            .copied()
            .unwrap_or(0)
    }

    /// Record a generation this machine opened a key of. Never lowers it.
    /// True when the number moved, which is when the file needs saving.
    pub fn accept_generation(&mut self, generation: i32) -> bool {
        let Some(server) = self.server_url.clone() else { return false };
        let seen = self.ork_generations.entry(server).or_insert(0);
        if generation > *seen {
            *seen = generation;
            true
        } else {
            false
        }
    }

    /// This machine has just made a new identity on `server`: an account from
    /// an invitation or a sign-up, with a key pair generated here a moment
    /// ago. Whatever was remembered for that address described an identity
    /// this one replaces (a server set up again on the same address is the
    /// usual case), so the count starts where the new one stands: 1 for
    /// somebody who created the organisation and its key, 0 for a member who
    /// holds no key yet.
    ///
    /// The one place the number may go down. It is safe here and nowhere
    /// else: no blob the server kept can be opened by a private key that did
    /// not exist until now.
    pub fn start_generation(&mut self, server: &str, generation: i32) {
        self.ork_generations.insert(server.to_string(), generation);
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

    fn pointed_at(server: &str) -> Settings {
        Settings { server_url: Some(server.into()), ..Default::default() }
    }

    #[test]
    fn a_generation_seen_on_one_server_says_nothing_about_another() {
        // The report of 10.11.2026: a key rotated on the old server, then a
        // new server whose organisation is at generation 1.
        let mut s = pointed_at("https://old.example");
        assert!(s.accept_generation(2));
        s.server_url = Some("https://new.example".into());
        assert_eq!(s.seen_generation(), 0);
        assert!(s.accept_generation(1));
        // And going back, the old server's number is still there.
        s.server_url = Some("https://old.example".into());
        assert_eq!(s.seen_generation(), 2);
    }

    #[test]
    fn a_generation_never_goes_back_by_being_accepted() {
        let mut s = pointed_at("https://a.example");
        assert!(s.accept_generation(3));
        assert!(!s.accept_generation(2));
        assert_eq!(s.seen_generation(), 3);
    }

    #[test]
    fn a_new_identity_starts_the_count_again() {
        // The same address, a server set up from nothing behind it.
        let mut s = pointed_at("https://a.example");
        s.accept_generation(4);
        s.start_generation("https://a.example", 1);
        assert_eq!(s.seen_generation(), 1);
    }

    #[test]
    fn the_number_an_older_version_kept_goes_to_its_server() {
        let mut s: Settings = serde_json::from_str(
            r#"{"server_url":null,"machine_id":"m","last_server":"https://a.example","ork_generation":3}"#,
        )
        .unwrap();
        s.adopt_legacy_generation();
        assert_eq!(s.ork_generation, 0);
        s.server_url = Some("https://a.example".into());
        assert_eq!(s.seen_generation(), 3);
        s.server_url = Some("https://b.example".into());
        assert_eq!(s.seen_generation(), 0);
    }

    #[test]
    fn a_machine_always_has_a_name() {
        assert!(!machine_name().trim().is_empty());
    }
}
