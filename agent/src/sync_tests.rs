// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! A profile bundle through a real server, and what the server ends up holding.
//!
//! `bundle.rs` already tests the sealing: a team bundle opens on a machine that
//! never packed it, the wrong key does not open it, a flipped byte refuses
//! rather than restoring something else. All of that is about the format.
//!
//! This is about the deployment. The claim that makes hosted mode possible is
//! not "the format is encrypted" — it is that a running server, having accepted
//! an upload and written it to disk, holds ciphertext and a key it cannot open.
//! That spans the agent, HTTP, the handler and the filesystem, and no unit test
//! of any one of them can make it.
//!
//! ```bash
//! FURY_TEST_SERVER=http://127.0.0.1:8899 \
//! FURY_TEST_TOKEN=<session token> \
//! FURY_TEST_PROFILE=<profile id> \
//! FURY_TEST_BUNDLE_DIR=<the server's FURY_BUNDLE_DIR> \
//!   cargo test -p fury-agent sync_tests -- --nocapture
//! ```
//!
//! Skipped, loudly, when those are absent.

use crate::bundle::{self, Sealer};

/// A string unmistakable in a hex dump if it ever reached disk in the clear.
const CANARY: &str = "SESSIONID=this-must-never-reach-the-server-in-the-clear";

/// The organisation key a team bundle is sealed under. Any 32 bytes; what
/// matters is that the server never sees them.
const ORG_KEY: [u8; 32] = [7u8; 32];

struct Env {
    server: crate::sync::Server,
    profile: String,
    bundle_dir: std::path::PathBuf,
}

fn env() -> Option<Env> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    Some(Env {
        server: crate::sync::Server {
            url: get("FURY_TEST_SERVER")?,
            token: get("FURY_TEST_TOKEN")?,
        },
        profile: get("FURY_TEST_PROFILE")?,
        bundle_dir: get("FURY_TEST_BUNDLE_DIR").map(std::path::PathBuf::from)?,
    })
}

impl Env {
    /// Take the lock this upload needs, rather than being handed one.
    ///
    /// Handed in from a shell it was a token that had already expired by the
    /// time the test ran, and the failure read as a bug in the upload path. A
    /// test that takes and returns its own says the same thing on the tenth run
    /// as on the first.
    async fn lock(&self) -> anyhow::Result<String> {
        let res = reqwest::Client::new()
            .post(format!("{}/v1/profiles/{}/lock", self.server.url, self.profile))
            .bearer_auth(&self.server.token)
            .json(&serde_json::json!({ "machine_id": "sync-e2e", "machine_name": "sync-e2e" }))
            .send()
            .await?;
        let status = res.status();
        let body: serde_json::Value = res.json().await.unwrap_or_default();
        body.get("lock_token")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("no lock ({status}): {body}"))
    }

    async fn unlock(&self, token: &str) {
        let _ = reqwest::Client::new()
            .post(format!("{}/v1/profiles/{}/unlock", self.server.url, self.profile))
            .bearer_auth(&self.server.token)
            .json(&serde_json::json!({ "lock_token": token }))
            .send()
            .await;
    }

    /// The version the server holds now, so an upload can name its base.
    async fn current_version(&self) -> i32 {
        self.server
            .fetch_bundle(&self.profile)
            .await
            .ok()
            .flatten()
            .map(|(_, _, v)| v)
            .unwrap_or(0)
    }
}

#[tokio::test]
async fn the_server_stores_ciphertext_and_gives_it_back_intact() {
    let Some(env) = env() else {
        eprintln!(
            "SKIPPED: set FURY_TEST_SERVER, FURY_TEST_TOKEN, FURY_TEST_PROFILE, FURY_TEST_BUNDLE_DIR"
        );
        return;
    };

    let lock = env.lock().await.expect("acquire a lock");
    let outcome = round_trip(&env, &lock).await;
    // Returned whatever happened: a lock left behind blocks the next run, and
    // the next run is usually somebody debugging why this one failed.
    env.unlock(&lock).await;
    outcome.expect("round trip");
}

async fn round_trip(env: &Env, lock: &str) -> anyhow::Result<()> {
    // A profile directory with something worth protecting in it. The canary
    // sits in a file named the way a real cookie jar is, so a server storing
    // the archive unsealed would show both the name and the value.
    let dir = crate::tmp::TempDir::new("fury-sync-e2e");
    let default = dir.join("Default");
    std::fs::create_dir_all(&default)?;
    std::fs::write(default.join("Cookies"), CANARY)?;
    std::fs::write(default.join("Preferences"), r#"{"profile":{"name":"acct"}}"#)?;

    // Sealed the way a TEAM profile is: under a key derived from the
    // organisation key and the profile id, not under this machine's key. That
    // is the case that matters — a bundle wrapped under a machine key opens
    // nowhere else, which is the opposite of the point of uploading it.
    let sealed = bundle::pack(&dir, &Sealer::Shared { key: ORG_KEY, vault: None })?;

    let base = env.current_version().await;
    let version = env
        .server
        .push_bundle(&env.profile, &sealed.bytes, &sealed.wrapped_key, &sealed.sha256, base, lock)
        .await?;
    anyhow::ensure!(
        version > base,
        "the server accepted the upload and did not advance the version"
    );

    // --- what is on the server's disk -------------------------------------
    //
    // Scoped to this profile's directory: a leftover from another run must not
    // be able to make these assertions pass or fail for the wrong reason.
    let here = env.bundle_dir.join(&env.profile);
    let stored: Vec<std::path::PathBuf> = std::fs::read_dir(&here)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", here.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    anyhow::ensure!(!stored.is_empty(), "the server wrote nothing to {}", here.display());
    eprintln!("server stored {} file(s) under {}", stored.len(), here.display());

    for path in &stored {
        let bytes = std::fs::read(path)?;
        anyhow::ensure!(
            !contains(&bytes, CANARY.as_bytes()),
            "{} holds the cookie in the clear",
            path.display()
        );
        // The archive's own structure too: "ustar" appears in every tar header,
        // and a gzip stream starts 1f 8b. Either would mean the server received
        // something it could walk, whatever else was true of the bytes.
        anyhow::ensure!(
            !contains(&bytes, b"ustar"),
            "{} holds a readable tar — the sealing did not happen",
            path.display()
        );
        anyhow::ensure!(
            bytes.len() < 2 || bytes[0..2] != [0x1f, 0x8b],
            "{} starts with a gzip header — the sealing did not happen",
            path.display()
        );
    }

    // --- and back again ----------------------------------------------------
    let (fetched, wrapped, got) = env
        .server
        .fetch_bundle(&env.profile)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the server had no bundle after accepting one"))?;
    anyhow::ensure!(got == version, "fetched version {got}, uploaded {version}");

    let restored = crate::tmp::TempDir::new("fury-sync-e2e-restore");
    bundle::unpack(&fetched, &wrapped, &Sealer::Shared { key: ORG_KEY, vault: None }, &restored)?;
    let back = std::fs::read_to_string(restored.join("Default/Cookies"))?;
    anyhow::ensure!(back == CANARY, "the round trip changed the data");

    // --- and not with anybody else's key -----------------------------------
    //
    // The other half of the claim: the ciphertext is opaque not only to the
    // server but to whoever takes a copy of its disk. Checked here rather than
    // in a test of its own, which would need this one to have run first and so
    // would pass or fail on cargo's scheduling.
    let wrong = crate::tmp::TempDir::new("fury-sync-e2e-wrongkey");
    let err = bundle::unpack(
        &fetched,
        &wrapped,
        &Sealer::Shared { key: [9u8; 32], vault: None },
        &wrong,
    )
    .expect_err("another organisation's key opened the bundle");
    eprintln!("a foreign key is refused: {err}");

    Ok(())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
