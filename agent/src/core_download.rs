// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Fetch the browser core from the project's releases, on request.
//!
//! This exists to delete a terminal step. Before it, installing Fury meant two
//! downloads and then
//!
//!     fury-agent install-core ~/Downloads/fury-core-*.tar.xz
//!
//! typed into a shell, which is a reasonable thing to ask of a developer and an
//! unreasonable thing to ask of somebody who came from a product where you
//! double-click an installer. The split into two artifacts stays -- the shell is
//! 12 MB and moves weekly, the core is 134 MB and moves when Chromium does, and
//! bundling them would mean a 134 MB download every time a button moved -- but
//! the second download is now a button rather than a command.
//!
//! WHAT THIS IS NOT, and the distinction is the whole reason it is allowed to
//! exist at all. The README says there are no automatic updates, deliberately:
//! an updater is a scheduled channel out of an anti-detect browser, from an
//! address that is not the profile's proxy. That objection is about
//! PERIODICITY. A machine that phones a known host every day at the same time
//! is a beacon, and "this address runs Fury and checked in at 14:03" is a
//! correlation handle for somebody running a hundred accounts.
//!
//! Nothing here runs on a timer, at startup, or in the background. It runs when
//! a person presses a button, and it makes exactly the requests that person
//! would have made with their browser had they gone to the releases page
//! themselves. That is a different thing from a beacon, and it is the only
//! version of this feature the project can have.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Mutex;

/// Where a download has got to, as the shell shows it.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Progress {
    /// True while a download is in flight. The shell uses this to keep the
    /// button disabled rather than tracking its own idea of busy.
    pub running: bool,
    pub downloaded: u64,
    /// Zero when the server did not send a length, which is why the shell shows
    /// a bar only when this is non-zero and bytes-so-far otherwise.
    pub total: u64,
    /// Set once, when it is over. Present with `running: false` means done.
    pub installed: Option<String>,
    /// Present with `running: false` means it failed, and this is why.
    pub error: Option<String>,
}

pub type Shared = Arc<Mutex<Progress>>;

/// The asset name this platform needs, as published.
///
/// Built from the same pieces package.sh uses, so a rename there has to happen
/// here too -- and a mismatch is visible immediately as "no core asset in the
/// latest release", which names both what it wanted and what was there.
fn asset_infix() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        #[cfg(target_arch = "aarch64")]
        {
            "macos-arm64"
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            "macos-x86_64"
        }
    }
    #[cfg(windows)]
    {
        "windows-x64"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "linux-x64"
    }
}

const RELEASES: &str =
    "https://api.github.com/repos/furyteamtop/fury-antidetect-browser/releases?per_page=10";

/// Name and URL of the newest core asset for this platform.
///
/// `/releases/latest` is deliberately not used: it excludes pre-releases, and
/// every release this project has made so far is one. Asking for the list and
/// taking the first entry gets the newest either way.
async fn newest_asset(client: &reqwest::Client) -> Result<(String, String)> {
    let releases: serde_json::Value = client
        .get(RELEASES)
        // GitHub rejects requests with no user agent. Naming the product and
        // version rather than pretending to be a browser: this request is the
        // agent asking for its own updates, and saying so is honest and also
        // what makes a rate-limit complaint traceable to us.
        .header("User-Agent", concat!("fury-agent/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("asking GitHub for the release list")?
        .error_for_status()
        .context("GitHub refused the release list")?
        .json()
        .await
        .context("reading the release list")?;

    let infix = asset_infix();
    let assets = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("assets")?.as_array());

    for release_assets in assets {
        for a in release_assets {
            let name = a.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            if name.starts_with("fury-core-") && name.contains(infix) {
                let url = a
                    .get("browser_download_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if !url.is_empty() {
                    return Ok((name.to_string(), url.to_string()));
                }
            }
        }
    }

    anyhow::bail!(
        "no core asset for {infix} in the latest releases. The releases page has \
         the file if this is wrong; install it with `fury-agent install-core <file>`"
    )
}

/// Download the newest core for this platform and install it.
///
/// Spawned rather than awaited by the caller: 134 MB is minutes, and an IPC
/// call that does not return for minutes is a shell that looks hung. Progress
/// goes into `shared`, which `status` reports.
pub fn start(shared: Shared) {
    tokio::spawn(async move {
        {
            let mut p = shared.lock().await;
            if p.running {
                // Two presses of the button are one download. Not an error --
                // the second press is somebody who did not see the first take.
                return;
            }
            *p = Progress {
                running: true,
                ..Default::default()
            };
        }

        let result = run(&shared).await;

        let mut p = shared.lock().await;
        p.running = false;
        match result {
            Ok(path) => p.installed = Some(path.display().to_string()),
            Err(e) => p.error = Some(format!("{e:#}")),
        }
    });
}

async fn run(shared: &Shared) -> Result<PathBuf> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60 * 30))
        .build()?;

    let (name, url) = newest_asset(&client).await?;
    tracing::info!(%name, "downloading the core");

    let mut response = client
        .get(&url)
        .header("User-Agent", concat!("fury-agent/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .context("starting the download")?
        .error_for_status()
        .context("the download was refused")?;

    {
        let mut p = shared.lock().await;
        p.total = response.content_length().unwrap_or(0);
    }

    // To a temp file rather than to memory: this is 134 MB, and a machine that
    // is already running several browsers should not have to hold it twice.
    let tmp = std::env::temp_dir().join(&name);
    let mut file = std::fs::File::create(&tmp)
        .with_context(|| format!("creating {}", tmp.display()))?;

    let mut seen: u64 = 0;
    while let Some(chunk) = response.chunk().await.context("while downloading")? {
        file.write_all(&chunk).context("while writing the download")?;
        seen += chunk.len() as u64;
        // Updated per chunk rather than per byte: the shell polls, so anything
        // finer is work nobody reads.
        shared.lock().await.downloaded = seen;
    }
    file.flush()?;
    drop(file);

    tracing::info!(bytes = seen, "downloaded; installing");

    // install() is the same path `fury-agent install-core` takes, deliberately:
    // it unpacks preserving symlinks, clears the quarantine flag the download
    // attracted, and RUNS THE BROWSER ONCE to check it starts. A download that
    // arrives corrupt should fail here, with the previous core still in place,
    // rather than at the operator's next launch.
    let installed = tokio::task::spawn_blocking(move || {
        let r = crate::install_core::install(&tmp);
        let _ = std::fs::remove_file(&tmp);
        r
    })
    .await??;

    Ok(installed)
}
