// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The geo database the exit check reads by default: DB-IP's City Lite.
//!
//! Why not ipinfo.io alone, which the check used to take country, zone and
//! coordinates from. ipinfo places an address where the server physically
//! is, and most datacenter proxies sold for a country sit somewhere else
//! with that country written into their registration. Measured 18.11.2026 on
//! a tester's proxies (one seller, Octo agreed with the seller): ipinfo said
//! US for 187.13.122.237 and 5.183.129.239 and HK for 136.234.163.215, while
//! MaxMind, ip-api, ipwho.is and DB-IP all said MX, CA and AU/JP. Sites and
//! anti-fraud read databases of that second kind, so a profile following
//! ipinfo got New York's clock on an exit every site places in Mexico.
//!
//! DB-IP Lite rather than MaxMind GeoLite2: it is licensed CC BY 4.0, which
//! allows fetching it for every user without an account or a licence key.
//! The attribution is in the About screen. It has no time zones, so the zone
//! comes from its coordinates (`geoip::zone_at`).
//!
//! Fetched straight from download.db-ip.com, not through any proxy: it is a
//! public file and asking for it says nothing about anybody's exits. Nothing
//! about an exit is sent anywhere new either; the exit IP is still learnt
//! from ipinfo.io through the proxy, and looked up here, on this machine.

use anyhow::Context as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// A month's file is published early in the month; older than this and a
/// newer one is looked for.
const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(32 * 24 * 3600);
/// The unpacked file is ~127 MB (November 2026). Anything far beyond that is
/// not the file we asked for.
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// The database, if one has been fetched. The check falls back to ipinfo.io
/// without it.
pub fn ready() -> Option<PathBuf> {
    let p = crate::paths::geo_db_file();
    p.is_file().then_some(p)
}

/// Fetch the database in the background, then look again once a day.
pub fn spawn_refresher() {
    tokio::spawn(async {
        // Not in the first seconds of a start, which belong to the window.
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        loop {
            if let Err(e) = refresh_if_stale().await {
                tracing::warn!(error = format!("{e:#}"), "the geo database was not refreshed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(24 * 3600)).await;
        }
    });
}

async fn refresh_if_stale() -> anyhow::Result<()> {
    let target = crate::paths::geo_db_file();
    let fresh = std::fs::metadata(&target)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < MAX_AGE);
    if fresh {
        return Ok(());
    }
    // This month's file, or last month's when this one is not out yet.
    let mut last_err = None;
    for month in candidate_months(time_now_ym()) {
        let url = format!("https://download.db-ip.com/free/dbip-city-lite-{month}.mmdb.gz");
        match fetch(&url, &target).await {
            Ok(()) => {
                tracing::info!(%url, "geo database updated");
                return Ok(());
            }
            Err(e) => last_err = Some(e.context(url)),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no month to try")))
}

/// Download, unpack beside the target, prove it reads, then swap it in. A
/// reader holding the old file keeps it; the next lookup opens the new one.
async fn fetch(url: &str, target: &Path) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(600))
        .user_agent(concat!("fury-agent/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut response = client.get(url).send().await?.error_for_status()?;
    let dir = target.parent().context("no parent directory")?;
    std::fs::create_dir_all(dir)?;
    let gz = dir.join("download.mmdb.gz");
    let mut file = std::fs::File::create(&gz)?;
    let mut seen = 0u64;
    while let Some(chunk) = response.chunk().await? {
        seen += chunk.len() as u64;
        anyhow::ensure!(seen <= MAX_BYTES, "larger than any geo database ({seen} bytes)");
        file.write_all(&chunk)?;
    }
    file.flush()?;
    drop(file);

    let target = target.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let tmp = target.with_extension("mmdb.new");
        let result = unpack_and_check(&gz, &tmp).and_then(|()| Ok(std::fs::rename(&tmp, &target)?));
        let _ = std::fs::remove_file(&gz);
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    })
    .await?
}

fn unpack_and_check(gz: &Path, out: &Path) -> anyhow::Result<()> {
    let mut reader = flate2::read::GzDecoder::new(std::fs::File::open(gz)?);
    let mut file = std::fs::File::create(out)?;
    let n = std::io::copy(&mut std::io::Read::take(&mut reader, MAX_BYTES + 1), &mut file)?;
    anyhow::ensure!(n <= MAX_BYTES, "larger than any geo database");
    file.flush()?;
    drop(file);
    // A file that opens and places a well-known address is the database.
    let geo = crate::geoip::lookup(&out.to_string_lossy(), "8.8.8.8")?;
    anyhow::ensure!(geo.country.as_deref() == Some("US"), "8.8.8.8 is not in the US in this file ({:?})", geo.country);
    Ok(())
}

/// "YYYY-MM" for now, in UTC.
fn time_now_ym() -> (i32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Civil from days (Howard Hinnant's algorithm), to avoid a date crate.
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m)
}

fn candidate_months((y, m): (i32, u32)) -> [String; 2] {
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    [format!("{y:04}-{m:02}"), format!("{py:04}-{pm:02}")]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_month_then_the_one_before() {
        assert_eq!(candidate_months((2026, 11)), ["2026-11".to_string(), "2026-10".to_string()]);
        assert_eq!(candidate_months((2027, 1)), ["2027-01".to_string(), "2026-12".to_string()]);
        let (y, m) = time_now_ym();
        assert!(y >= 2026 && (1..=12).contains(&m), "{y}-{m}");
    }
}
