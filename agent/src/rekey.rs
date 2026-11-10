// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Moving a local profile's cookies onto the key that travels with it.
//!
//! # The bug this exists to fix
//!
//! A local profile's cookies are encrypted under this machine's key: the Safe
//! Storage keychain item on macOS, a DPAPI-wrapped key on Windows. Each value
//! on disk starts with "v10". A team profile's are encrypted under a key
//! derived from the profile's own (patch 0110), and start with "f10".
//!
//! Uploading a local profile to a team packed the directory as it stood, v10
//! rows and all. The core does not convert a row it merely reads: a value is
//! written under the new key only when a site sets that cookie again
//! (sqlite_persistent_cookie_store.cc never looks at `should_reencrypt`). So
//! every cookie the site had not touched since stayed bound to the machine
//! that made it, a colleague's browser could not open them, and Chromium
//! answers an unreadable row by deleting the site's whole group. Reported as
//! "logged out on the other machine", 09.11.2026, issue 26.
//!
//! # How
//!
//! Through the browser, for the reasons cookies.rs gives: nothing outside it
//! can be trusted to read or write that file. A copy of the jar goes into an
//! empty directory, the core is started there with both keys in hand (the
//! machine's, which it always has, and the profile's, which it is given), and
//! every cookie is read out and set again over CDP. Reading uses whichever key
//! the row names; writing uses the profile's.
//!
//! No window, no page and no network: the browser is headless, opens
//! about:blank, and its proxy is a port that never answers. The profile on
//! disk is not touched. The rewritten jar is handed back as a file, and the
//! upload packs it in place of the original (`bundle::pack_with`).
//!
//! What does not survive: each cookie's creation and last-access time, which
//! become the moment of the conversion. Their relative order is kept, because
//! the browser returns them in the order it sends them and they are set back
//! in that order.

use std::path::{Path, PathBuf};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Row};

/// What the profile's key provider writes in front of a value. Patch 0110,
/// `kFuryKeyTag`; on disk in every team profile, so it never changes.
const TAG: &str = "f10";

/// Where the jar is, relative to the profile directory. The second is where
/// Chromium kept it before M96 and where it still looks to migrate from.
const JARS: &[&str] = &["Default/Network/Cookies", "Default/Cookies"];

/// The file the agent keeps beside a team profile's data: the fingerprint of
/// the key its cookies are under (ipc.rs, `err.cookieKeyChanged`).
pub const KEY_MARKER: &str = "Fury Key Id";

/// The jar of this profile, as a path relative to it, if it has one.
pub fn jar(profile_dir: &Path) -> Option<PathBuf> {
    JARS.iter()
        .map(PathBuf::from)
        .find(|rel| profile_dir.join(rel).is_file())
}

/// How many cookies a jar holds, and how many of them only this machine can
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Census {
    pub rows: i64,
    pub machine_bound: i64,
}

/// Count them, without the browser and without a key: the three bytes in
/// front of each value say which key it is under, and that is all this reads.
pub async fn census(jar: &Path) -> anyhow::Result<Census> {
    let mut db = SqliteConnectOptions::new()
        .filename(jar)
        .read_only(true)
        .connect()
        .await?;
    let row = sqlx::query(
        "SELECT COUNT(*) AS rows, \
                COALESCE(SUM(CASE WHEN length(encrypted_value) > 0 \
                                   AND substr(CAST(encrypted_value AS BLOB), 1, 3) != CAST(?1 AS BLOB) \
                              THEN 1 ELSE 0 END), 0) AS bound \
         FROM cookies",
    )
    .bind(TAG)
    .fetch_one(&mut db)
    .await?;
    use sqlx::Connection;
    let census = Census { rows: row.try_get("rows")?, machine_bound: row.try_get("bound")? };
    db.close().await?;
    Ok(census)
}

/// What the browser needs in order to start at all.
pub struct Core<'a> {
    pub binary: &'a Path,
    /// A config the core accepts. Nothing reads the fingerprint here, because
    /// no page loads; it is passed because the core refuses to start without.
    pub config: &'a serde_json::Value,
    pub ui_locale: &'a str,
}

/// A directory that removes itself, for the browser's short visit.
pub struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("fury-rekey-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A rewritten jar, and the directory that keeps it alive.
pub struct Rekeyed {
    /// Holds `jar` and `marker`; they go when this does.
    _scratch: Scratch,
    /// The jar, every row under the profile's key.
    pub jar: PathBuf,
    /// A file holding the key's fingerprint, to travel as `KEY_MARKER`.
    pub marker: PathBuf,
    /// Cookies the browser read from the old jar.
    pub read: usize,
    /// Of those, the ones it refused to take back. Named in the log.
    pub refused: usize,
}

/// The browser, for as long as the exchange lasts.
///
/// Killed when dropped: an early return must not leave a headless Chrome
/// behind holding a copy of somebody's cookies.
struct Browser {
    child: std::process::Child,
    ws: String,
    /// The port the browser was told is its proxy. Bound and never accepted
    /// on, so anything it tries to fetch waits and goes nowhere.
    _nowhere: std::net::TcpListener,
}

impl Browser {
    async fn start(core: &Core<'_>, dir: &Path, key: Option<[u8; 32]>) -> anyhow::Result<Self> {
        let nowhere = std::net::TcpListener::bind("127.0.0.1:0")?;
        let urls = ["about:blank".to_string()];
        let child = crate::launcher::spawn(&crate::launcher::LaunchSpec {
            dock_name: None,
            core_binary: core.binary,
            user_data_dir: dir,
            config: core.config,
            relay_port: nowhere.local_addr()?.port(),
            extensions: &[],
            restrictions: fury_shared::rbac::LaunchRestrictions::for_perms(
                fury_shared::rbac::PermSet::full_profile_work(),
            ),
            start_urls: &urls,
            os_crypt_key: key,
            ui_locale: core.ui_locale,
            debug_port: Some(0),
            background: false,
            headless: true,
        })?;
        let mut browser = Self { child, ws: String::new(), _nowhere: nowhere };
        let (_, ws) = crate::ipc::devtools_endpoint(dir).await.ok_or_else(|| {
            anyhow::anyhow!("the browser started and never opened a debugging endpoint")
        })?;
        browser.ws = ws;
        Ok(browser)
    }

    /// Ask it to quit and wait until it has: the jar is committed on the way
    /// out, and a jar copied before that is a jar missing its last writes.
    async fn close(mut self) -> anyhow::Result<()> {
        if let Ok(mut cdp) = crate::cookies::Cdp::connect(&self.ws).await {
            // The reply may never come: the browser is closing the socket it
            // would be sent on.
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                cdp.call("Browser.close", serde_json::json!({})),
            )
            .await;
        }
        for _ in 0..300 {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        anyhow::bail!("the browser did not quit within thirty seconds of being asked")
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Make a directory holding only a jar pass for a profile that has run before.
///
/// Session cookies (the ones with no expiry) are kept across runs because the
/// launcher passes --restore-last-session, and Chromium honours that switch
/// only for a profile that is not new, which it decides by whether a
/// preferences file was there to read. In a new one it deletes them from the
/// jar on the way in. Measured 10.11.2026 on Windows: four cookies in the
/// jar, one of them a session cookie, three read back.
fn not_new(dir: &Path) -> std::io::Result<()> {
    let prefs = dir.join("Default").join("Preferences");
    if !prefs.exists() {
        std::fs::create_dir_all(dir.join("Default"))?;
        std::fs::write(prefs, b"{}")?;
    }
    Ok(())
}

/// Rewrite a profile's jar under `key`, in a copy.
///
/// `profile_dir` is read and never written. Fails rather than returning a jar
/// that still holds a row the next machine cannot open: the caller is about to
/// send it to that machine.
pub async fn rekey(profile_dir: &Path, core: &Core<'_>, key: [u8; 32]) -> anyhow::Result<Option<Rekeyed>> {
    let Some(rel) = jar(profile_dir) else {
        return Ok(None);
    };
    let before = census(&profile_dir.join(&rel)).await?;
    if before.machine_bound == 0 {
        return Ok(None);
    }

    let scratch = Scratch::new()?;
    let dir = scratch.0.join("profile");
    let copy = dir.join(&rel);
    std::fs::create_dir_all(copy.parent().expect("a jar path has a parent"))?;
    std::fs::copy(profile_dir.join(&rel), &copy)?;
    // On Windows the machine's key is not in a keychain but in this file,
    // wrapped by DPAPI. Without it the browser makes a new one and reads
    // nothing.
    let local_state = profile_dir.join("Local State");
    if local_state.is_file() {
        std::fs::copy(&local_state, dir.join("Local State"))?;
    }
    not_new(&dir)?;

    let browser = Browser::start(core, &dir, Some(key)).await?;
    let exchange = async {
        let cookies = crate::cookies::export(&browser.ws).await?;
        let prepared = crate::cookies::prepare(&cookies)?;
        let mut cdp = crate::cookies::Cdp::connect(&browser.ws).await?;
        // Cleared first, so that every row is written anew. Setting a cookie
        // over an identical one is allowed to be a no-op.
        cdp.call("Storage.clearCookies", serde_json::json!({})).await?;
        let mut refused = 0usize;
        // One batch when it goes; one by one when it does not, because a batch
        // fails whole and a single cookie the browser dislikes must not cost
        // the other four hundred.
        if !prepared.is_empty()
            && cdp
                .call("Storage.setCookies", serde_json::json!({ "cookies": prepared }))
                .await
                .is_err()
        {
            cdp.call("Storage.clearCookies", serde_json::json!({})).await?;
            for cookie in &prepared {
                if let Err(e) = cdp
                    .call("Storage.setCookies", serde_json::json!({ "cookies": [cookie] }))
                    .await
                {
                    refused += 1;
                    tracing::warn!(
                        domain = cookie.get("domain").and_then(|v| v.as_str()).unwrap_or(""),
                        name = cookie.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        error = %e,
                        "a cookie did not move to the profile's key"
                    );
                }
            }
        }
        anyhow::Ok((cookies.len(), refused))
    }
    .await;
    // Closed before the result is looked at: either way the browser has to go.
    let closed = browser.close().await;
    let (read, refused) = exchange?;
    closed?;

    let after = census(&copy).await?;
    if after.machine_bound > 0 {
        anyhow::bail!(
            "{} of {} cookies are still under this machine's key after the conversion",
            after.machine_bound,
            after.rows
        );
    }
    let expected = (read - refused) as i64;
    if after.rows < expected {
        anyhow::bail!(
            "the browser read {read} cookies and wrote back {}, so some were lost on the way",
            after.rows
        );
    }

    let marker = scratch.0.join("key-id");
    std::fs::write(&marker, fury_shared::keys::key_fingerprint(&key))?;
    tracing::info!(
        rows_before = before.rows,
        machine_bound = before.machine_bound,
        read,
        refused,
        rows_after = after.rows,
        "moved the cookie jar onto the profile's key"
    );
    Ok(Some(Rekeyed { _scratch: scratch, jar: copy, marker, read, refused }))
}

impl Rekeyed {
    /// What `bundle::pack_with` has to swap for this jar to travel in place of
    /// the profile's own.
    pub fn swaps(&self, profile_dir: &Path) -> Vec<crate::bundle::Swap> {
        let rel = jar(profile_dir).expect("a rekeyed profile has a jar");
        let mut journal = rel.clone().into_os_string();
        journal.push("-journal");
        vec![
            crate::bundle::Swap { path: rel, from: Some(self.jar.clone()) },
            // The old jar's journal describes the old jar.
            crate::bundle::Swap { path: journal.into(), from: None },
            crate::bundle::Swap { path: KEY_MARKER.into(), from: Some(self.marker.clone()) },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmp::TempDir;

    async fn jar_with(dir: &Path, values: &[&[u8]]) -> PathBuf {
        let path = dir.join("Default/Network/Cookies");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut db = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        sqlx::query("CREATE TABLE cookies (host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB)")
            .execute(&mut db)
            .await
            .unwrap();
        for v in values {
            sqlx::query("INSERT INTO cookies VALUES ('.example.com', 'n', '', ?1)")
                .bind(*v)
                .execute(&mut db)
                .await
                .unwrap();
        }
        use sqlx::Connection;
        db.close().await.unwrap();
        path
    }

    #[tokio::test]
    async fn the_census_tells_a_machines_rows_from_the_profiles() {
        let dir = TempDir::new("rekey-census");
        // The four shapes a jar holds: the keychain's, the app-bound Windows
        // one, the profile's, and a value from before values were encrypted.
        let jar = jar_with(&dir, &[b"v10\x01\x02", b"v20\x01", b"f10\x01\x02\x03", b""]).await;
        assert_eq!(census(&jar).await.unwrap(), Census { rows: 4, machine_bound: 2 });
    }

    #[tokio::test]
    async fn a_jar_already_on_the_profiles_key_starts_no_browser() {
        let dir = TempDir::new("rekey-nothing");
        jar_with(&dir, &[b"f10\x01", b"f10\x02"]).await;
        let config = serde_json::json!({});
        // A core that does not exist: reaching for it would be the failure.
        let core = Core { binary: Path::new("/nonexistent/Fury"), config: &config, ui_locale: "en-US" };
        assert!(rekey(&dir, &core, [7; 32]).await.unwrap().is_none());
        // And neither does a profile that was never opened.
        let empty = TempDir::new("rekey-empty");
        assert!(rekey(&empty, &core, [7; 32]).await.unwrap().is_none());
    }

    /// The whole thing against a real core, which is the only way to know any
    /// of it: a jar written under the machine's key, converted, and read back
    /// by a browser that has the profile's key and a different machine key.
    ///
    ///     FURY_TEST_CORE=<chrome> FURY_TEST_PERSONA=<persona.json> \
    ///         cargo test -p fury-agent rekey -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "needs a core: set FURY_TEST_CORE and FURY_TEST_PERSONA"]
    async fn a_local_jar_reads_back_under_the_profiles_key_on_another_machine() {
        let binary = PathBuf::from(std::env::var("FURY_TEST_CORE").expect("FURY_TEST_CORE"));
        let persona: fury_shared::persona::Persona = serde_json::from_slice(
            &std::fs::read(std::env::var("FURY_TEST_PERSONA").expect("FURY_TEST_PERSONA")).unwrap(),
        )
        .unwrap();
        let config = persona.derive_core_config(
            1,
            &fury_shared::persona::ProfileContext {
                timezone: "UTC".into(),
                languages: vec!["en-US".into(), "en".into()],
                ui_locale: "en-US".into(),
                geolocation: None,
                chrome_major: crate::CHROME_MAJOR,
                chrome_full_version: crate::CHROME_FULL_VERSION.to_string(),
            },
        );
        let core = Core { binary: &binary, config: &config, ui_locale: "en-US" };
        let key = [42u8; 32];
        let far = 4_102_444_800.0; // 2100

        // A local profile: no key, so the machine's.
        let local = TempDir::new("rekey-local");
        let wanted = serde_json::json!([
            { "name": "sid", "value": "persistent-value", "domain": ".example.com", "path": "/",
              "secure": true, "httpOnly": true, "sameSite": "Lax", "expires": far },
            { "name": "host", "value": "host-only", "domain": "login.example.org", "path": "/a",
              "secure": true, "expires": far },
            { "name": "part", "value": "partitioned", "domain": "widget.example.net", "path": "/",
              "secure": true, "sameSite": "None", "expires": far,
              "partitionKey": { "topLevelSite": "https://top.example", "hasCrossSiteAncestor": true } },
            { "name": "sess", "value": "session-value", "domain": ".example.com", "path": "/" },
        ]);
        let browser = Browser::start(&core, &local, None).await.unwrap();
        crate::cookies::import(&browser.ws, wanted.as_array().unwrap()).await.unwrap();
        browser.close().await.unwrap();

        let rel = jar(&local).expect("the browser wrote a jar");
        let before = census(&local.join(&rel)).await.unwrap();
        println!("local jar: {before:?}");
        assert!(before.rows >= 3, "{before:?}");
        assert_eq!(before.machine_bound, before.rows, "a local jar is all under the machine's key");

        let rekeyed = rekey(&local, &core, key).await.unwrap().expect("there was something to convert");
        println!("read {} refused {}", rekeyed.read, rekeyed.refused);
        assert_eq!(rekeyed.refused, 0);
        let after = census(&rekeyed.jar).await.unwrap();
        println!("rekeyed jar: {after:?}");
        assert_eq!(after.machine_bound, 0);
        assert_eq!(after.rows, before.rows, "no cookie was lost");
        // The original is as it was.
        assert_eq!(census(&local.join(&rel)).await.unwrap(), before);

        // Another machine: the jar and nothing else. No Local State, so on
        // Windows a machine key this jar has never seen.
        let other = TempDir::new("rekey-other");
        std::fs::create_dir_all(other.join(&rel).parent().unwrap()).unwrap();
        std::fs::copy(&rekeyed.jar, other.join(&rel)).unwrap();
        not_new(&other).unwrap();
        let browser = Browser::start(&core, &other, Some(key)).await.unwrap();
        let got = crate::cookies::export(&browser.ws).await.unwrap();
        browser.close().await.unwrap();
        let value = |name: &str| {
            got.iter()
                .find(|c| c["name"] == name)
                .and_then(|c| c["value"].as_str())
                .map(str::to_string)
        };
        println!("read back: {}", serde_json::to_string(&got).unwrap());
        assert_eq!(value("sid").as_deref(), Some("persistent-value"));
        assert_eq!(value("host").as_deref(), Some("host-only"));
        assert_eq!(value("part").as_deref(), Some("partitioned"));
        assert_eq!(value("sess").as_deref(), Some("session-value"));
        assert_eq!(got.len() as i64, before.rows);
        let part = got.iter().find(|c| c["name"] == "part").unwrap();
        assert_eq!(part["partitionKey"]["topLevelSite"], "https://top.example");

        // And the control: the same jar without the key is what a colleague's
        // browser got before this existed. It must NOT read, or the test above
        // proves nothing about the key.
        let control = TempDir::new("rekey-control");
        std::fs::create_dir_all(control.join(&rel).parent().unwrap()).unwrap();
        std::fs::copy(&rekeyed.jar, control.join(&rel)).unwrap();
        not_new(&control).unwrap();
        let browser = Browser::start(&core, &control, Some([1u8; 32])).await.unwrap();
        let got = crate::cookies::export(&browser.ws).await.unwrap();
        browser.close().await.unwrap();
        assert!(got.is_empty(), "a wrong key read {} cookies", got.len());
    }
}
