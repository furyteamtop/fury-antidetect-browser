// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Is there a newer version, and installing it.
//!
//! The check asks the release feed and reports. The install (from 0.2.25) is
//! Tauri's updater: it downloads the build named in `latest.json` on the
//! newest release, checks its minisign signature against the public key in
//! tauri.conf.json, and only then replaces the application. Installing an
//! update is running code the person did not choose, so the signature is the
//! whole point: the private key lives on the release machine
//! (~/.private_keys/fury-updater.key) and nothing that reaches GitHub, the
//! network or this process can make an unsigned build install.
//!
//! What a person sees: one button. If profiles are open it says to close them
//! (the agent holds their locks and relays). Otherwise the agent is stopped,
//! the build downloads with progress, installs, and the application restarts
//! on the new version. A release without a signed build falls back to the
//! download link, which is all the button did before.

use serde::Serialize;
use std::sync::Mutex;
use tauri_plugin_updater::UpdaterExt;

/// Where releases are published. The repository is also in Cargo.toml; here it
/// is the API host, so a fork changes one constant.
//
// The LIST, not `/releases/latest`, and the difference is not cosmetic:
// `/releases/latest` excludes pre-releases and answers 404 when every release
// is one. This project's releases are all pre-releases today, so the endpoint
// that sounds right would have told every user "nothing has been published"
// while two releases sat on the page. Measured 16.08.2026, the day the second
// one went up. The list is newest-first, so the first entry is the answer.
const RELEASES: &str = "https://api.github.com/repos/furyteamtop/fury-antidetect-browser/releases?per_page=5";

#[derive(Serialize)]
pub struct UpdateCheck {
    pub current: &'static str,
    /// `None` when nothing has been published yet, which is not an error.
    pub latest: Option<String>,
    pub url: Option<String>,
    /// The installer for this platform in that release, so the bar's button
    /// downloads the file rather than leaving the person on a page of six.
    pub download: Option<String>,
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

/// The asset a person on this machine installs: the dmg on a Mac, the setup
/// on Windows. Never the core archives, which the application fetches itself.
fn installer(release: &serde_json::Value) -> Option<String> {
    let suffix = if cfg!(target_os = "macos") {
        "-macos-arm64.dmg"
    } else if cfg!(windows) {
        "-windows-x64-setup.exe"
    } else {
        return None;
    };
    release
        .get("assets")?
        .as_array()?
        .iter()
        .find(|a| a.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.ends_with(suffix)))?
        .get("browser_download_url")?
        .as_str()
        .filter(|u| u.starts_with("https://"))
        .map(str::to_string)
}

#[tauri::command]
pub async fn check_update(state: tauri::State<'_, crate::commands::AppState>) -> Result<UpdateCheck, crate::commands::ApiErr> {
    let unreachable = |message: String| UpdateCheck {
        current: current(),
        latest: None,
        url: None,
        download: None,
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
            download: None,
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

    // The feed is an array now. An empty one is a repository with no releases,
    // which is the same honest "unpublished" the 404 used to mean.
    let newest = body.as_array().and_then(|a| a.first());
    let body = match newest {
        Some(r) => r.clone(),
        None => {
            return Ok(UpdateCheck {
                current: current(),
                latest: None,
                url: None,
                download: None,
                notes: None,
                status: "unpublished",
                message: None,
            })
        }
    };

    let latest = body.get("tag_name").and_then(|v| v.as_str()).unwrap_or_default();
    if latest.is_empty() {
        return Ok(UpdateCheck {
            current: current(),
            latest: None,
            url: None,
            download: None,
            notes: None,
            status: "unpublished",
            message: None,
        });
    }

    Ok(UpdateCheck {
        current: current(),
        latest: Some(latest.to_string()),
        url: body.get("html_url").and_then(|v| v.as_str()).map(str::to_string),
        download: installer(&body),
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

/// Open a release page in the operator's own browser.
///
/// Tauri needs this because an <a target="_blank"> inside the application window
/// has nowhere to go: the window is not a browser and has no tab to open. The
/// link in Settings -> About looked like a link, and did nothing at all, until
/// somebody pressed it and said so.
///
/// No plugin for fifteen lines, and no shell either. `open` and rundll32 take
/// the URL as an argument rather than as part of a command line a shell will
/// re-parse, so there is no quoting to get wrong and nothing to inject into.
///
/// The scheme is checked because the argument arrives from a network response.
/// The release feed is ours today, and a `file:` or `javascript:` URL from a
/// feed that stopped being ours is exactly the kind of thing that should fail
/// here rather than be handed to the operating system.
#[tauri::command]
pub async fn open_url(url: String) -> Result<(), crate::commands::ApiErr> {
    if !url.starts_with("https://") {
        return Err(crate::commands::ApiErr::local(format!(
            "refusing to open {url}: only https links"
        )));
    }

    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(&url).spawn();

    #[cfg(windows)]
    let spawned = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(&url).spawn();

    spawned
        .map(|_| ())
        .map_err(|e| crate::commands::ApiErr::local(format!("could not open a browser: {e}")))
}

/// Where an install is, for the bar to show. Polled, like the core download.
#[derive(Clone, Default, Serialize)]
pub struct InstallProgress {
    /// `"idle" | "checking" | "stopping" | "downloading" | "installing" | "restarting" | "failed"`
    pub stage: &'static str,
    pub received: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
}

static PROGRESS: Mutex<InstallProgress> =
    Mutex::new(InstallProgress { stage: "idle", received: 0, total: None, error: None });

fn set_stage(stage: &'static str) {
    if let Ok(mut p) = PROGRESS.lock() {
        p.stage = stage;
    }
}

#[tauri::command]
pub fn update_progress() -> InstallProgress {
    PROGRESS.lock().map(|p| p.clone()).unwrap_or_default()
}

/// A failure the bar names in the person's language; the message is the
/// fallback.
fn coded(code: &str, message: impl Into<String>) -> crate::commands::ApiErr {
    let mut e = crate::commands::ApiErr::local(message);
    e.code = Some(code.to_string());
    e
}

/// Download, verify, install and restart. See the module notes.
///
/// Errors with code `profiles_open` (nothing was touched), `no_signed_update`
/// (the newest release has no signed build: the bar opens the download link
/// instead) or `update_failed`.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), crate::commands::ApiErr> {
    let result = install(&app).await;
    if let Err(e) = &result {
        if let Ok(mut p) = PROGRESS.lock() {
            p.stage = "failed";
            p.error = Some(e.message.clone());
        }
    }
    result
}

async fn install(app: &tauri::AppHandle) -> Result<(), crate::commands::ApiErr> {
    if let Ok(mut p) = PROGRESS.lock() {
        *p = InstallProgress { stage: "checking", ..Default::default() };
    }

    // Open profiles first: stopping the agent under them would drop their
    // locks and relays, and the installer on Windows kills it outright.
    if let Ok(status) = crate::agent::call::<serde_json::Value>("status", serde_json::json!({})).await {
        let open = status["running"].as_array().map_or(0, Vec::len);
        if open > 0 {
            return Err(coded("profiles_open", format!("{open} profile(s) are open; close them first")));
        }
    }

    // FURY_UPDATE_FEED points the check at another latest.json, for testing an
    // install end to end. It cannot weaken anything: whatever it names still
    // has to carry a signature from the release key.
    let mut builder = app.updater_builder();
    if let Ok(feed) = std::env::var("FURY_UPDATE_FEED") {
        let url = feed
            .parse()
            .map_err(|e| coded("update_failed", format!("FURY_UPDATE_FEED: {e}")))?;
        builder = builder
            .endpoints(vec![url])
            .map_err(|e| coded("update_failed", e.to_string()))?;
    }
    let update = builder
        .build()
        .map_err(|e| coded("update_failed", e.to_string()))?
        .check()
        .await
        .map_err(|e| coded("update_failed", e.to_string()))?
        .ok_or_else(|| coded("no_signed_update", "the newest release has no signed build for this system"))?;

    // Downloaded and checked against the release key before anything else is
    // touched: a build that fails here leaves the agent running and the
    // application as it was. Measured on a test bundle offered a tampered
    // archive: refused, the bundle unchanged byte for byte.
    set_stage("downloading");
    let bytes = update
        .download(
            |chunk, total| {
                if let Ok(mut p) = PROGRESS.lock() {
                    p.received += chunk as u64;
                    p.total = total;
                }
            },
            || {},
        )
        .await
        .map_err(|e| coded("update_failed", e.to_string()))?;

    // The agent runs from inside the application: on macOS the bundle is
    // replaced under it, and it would go on serving the old code until the
    // machine restarts. An agent older than 0.2.25 does not know the method;
    // that is not a reason to stop (Windows' installer kills it anyway).
    set_stage("stopping");
    match crate::agent::call::<serde_json::Value>("agent.shutdown", serde_json::json!({})).await {
        Ok(_) => {
            for _ in 0..30 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if fury_platform::Stream::connect(&crate::agent::ipc_endpoint()).await.is_err() {
                    break;
                }
            }
        }
        Err(e) => eprintln!("agent did not stop before the update: {e}"),
    }

    set_stage("installing");
    if let Err(e) = update.install(bytes) {
        // Nothing replaced: bring the agent back for the version still here.
        let _ = crate::agent::ensure_running().await;
        return Err(coded("update_failed", e.to_string()));
    }

    // On Windows the installer has already taken over and this process is on
    // its way out; on macOS the new bundle is in place and this restarts into
    // it, and the new shell starts the new agent.
    set_stage("restarting");
    app.restart();
}

/// FURY_UPDATE_INSTALL_NOW with FURY_UPDATE_FEED: install at start, for an end
/// to end test on a machine nobody is clicking on. Same checks, same signature.
pub fn maybe_install_on_start(app: &tauri::AppHandle) {
    if std::env::var("FURY_UPDATE_INSTALL_NOW").is_err() || std::env::var("FURY_UPDATE_FEED").is_err() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if let Err(e) = install(&app).await {
            eprintln!("install on start failed: {} ({:?})", e.message, e.code);
        }
    });
}
