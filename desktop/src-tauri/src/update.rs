// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Is there a newer version?
//!
//! Asks the release feed and reports; it does not install. Installing an update
//! means running code the user did not choose, so it has to be verifiable — a
//! signature they can check against a key shipped with the application — and
//! until releases are signed, an automatic installer would be a way to push
//! arbitrary code onto every machine running Fury. That is precisely the
//! property an anti-detect browser must not have.
//!
//! So: check, tell, link. The install stays a decision a person makes.
//!
//! # Why not the Tauri updater plugin
//!
//! Because it wants the same thing this file is waiting for — a signing key and
//! a signed feed — and adds an installer on top. When the keys exist the plugin
//! becomes the right answer and this becomes its check step.

use serde::Serialize;

/// Where releases are published. The repository is also in Cargo.toml; here it
/// is the API host, so a fork changes one constant.
const RELEASES: &str = "https://api.github.com/repos/furyteamtop/fury-antidetect-browser/releases/latest";

#[derive(Serialize)]
pub struct UpdateCheck {
    pub current: &'static str,
    /// `None` when nothing has been published yet, which is not an error.
    pub latest: Option<String>,
    pub url: Option<String>,
    pub notes: Option<String>,
    /// `"current" | "available" | "unpublished" | "unreachable"`
    pub status: &'static str,
    pub message: Option<String>,
}

fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Compare two dotted versions numerically.
///
/// String comparison would call 0.10.0 older than 0.9.0, which is the classic
/// way for an update check to go quiet exactly when it matters. Anything
/// unparseable sorts as 0 rather than failing the check.
fn newer(latest: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches(['v', 'V'])
            // Drop any pre-release suffix: 1.2.0-rc.1 compares as 1.2.0.
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(latest), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

#[tauri::command]
pub async fn check_update(state: tauri::State<'_, crate::commands::AppState>) -> Result<UpdateCheck, crate::commands::ApiErr> {
    let unreachable = |message: String| UpdateCheck {
        current: current(),
        latest: None,
        url: None,
        notes: None,
        status: "unreachable",
        message: Some(message),
    };

    let res = state
        .http
        .get(RELEASES)
        // GitHub refuses anonymous requests without one, and a version in it
        // makes a support question answerable from the server's own logs.
        .header("User-Agent", format!("Fury/{}", current()))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;

    let res = match res {
        Ok(r) => r,
        // Not an error dialog. A machine behind a captive portal, or one
        // deliberately kept off the internet, is a normal way to run this.
        Err(e) => return Ok(unreachable(format!("Could not reach the release feed: {e}"))),
    };

    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateCheck {
            current: current(),
            latest: None,
            url: None,
            notes: None,
            status: "unpublished",
            message: None,
        });
    }
    if !res.status().is_success() {
        return Ok(unreachable(format!("The release feed answered {}", res.status())));
    }

    let body: serde_json::Value = match res.json().await {
        Ok(v) => v,
        Err(e) => return Ok(unreachable(format!("The release feed was unreadable: {e}"))),
    };

    let latest = body.get("tag_name").and_then(|v| v.as_str()).unwrap_or_default();
    if latest.is_empty() {
        return Ok(UpdateCheck {
            current: current(),
            latest: None,
            url: None,
            notes: None,
            status: "unpublished",
            message: None,
        });
    }

    Ok(UpdateCheck {
        current: current(),
        latest: Some(latest.to_string()),
        url: body.get("html_url").and_then(|v| v.as_str()).map(str::to_string),
        notes: body.get("body").and_then(|v| v.as_str()).map(str::to_string),
        status: if newer(latest, current()) { "available" } else { "current" },
        message: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers_not_as_text() {
        // The one that matters: alphabetically "0.10.0" < "0.9.0".
        assert!(newer("0.10.0", "0.9.0"));
        assert!(!newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn the_same_version_is_not_an_update() {
        assert!(!newer("1.2.3", "1.2.3"));
        assert!(!newer("v1.2.3", "1.2.3"), "a v prefix is not a new version");
    }

    #[test]
    fn shorter_versions_still_compare() {
        assert!(newer("1.1", "1.0.9"));
        assert!(!newer("1.0", "1.0.0"));
    }

    #[test]
    fn a_release_candidate_does_not_beat_its_own_release() {
        assert!(!newer("1.2.0-rc.1", "1.2.0"));
        assert!(newer("1.2.0-rc.1", "1.1.9"));
    }

    #[test]
    fn nonsense_does_not_announce_an_update() {
        assert!(!newer("banana", "0.0.1"));
    }
}
