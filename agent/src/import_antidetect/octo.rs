// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Octo Browser, through its cloud API (app.octobrowser.net/api/v2/automation,
//! docs.octobrowser.net/api/) for the profiles, and its Local Client API
//! (localhost:58888, the desktop app) for the cookies. The token is in Octo:
//! Settings, API. Every answer of the cloud API is `{success, msg, data, code}`.
//!
//! - `GET /profiles` returns only `uuid` unless `fields` names the rest, and
//!   never the fingerprint: the OS is in `GET /profiles/{uuid}`, which the
//!   import reads for each profile anyway, so the list stays one call per
//!   hundred profiles and the OS travels in `Detail::os`.
//! - Tags are names, not ids. A proxy is either a saved one (`uuid` set) or a
//!   temporary one bound to the profile (`uuid` empty); both carry `type`,
//!   `host`, `port`, `login`, `password` in clear text. When a saved proxy
//!   comes without its password, `GET /proxies` has it.
//! - **The cloud API has no cookie export.** The only way out is the profile
//!   itself: the Local Client API starts it headless with a debugging port,
//!   `Storage.getCookies` reads the jar over CDP (cookies.rs), and the profile
//!   is stopped again. So cookies need the Octo app running and signed in on
//!   this machine; without it the profile comes over without them and
//!   `cookie_note` says why. A profile already open in Octo is read where it
//!   runs if Octo gave it a port, and left open.
//! - Budget: Octo counts start and stop against the same per-minute and
//!   per-hour limit as the cloud calls (Base: 50 a minute, 500 an hour), so a
//!   profile costs three. A 429 stops the run with a sentence (mod.rs).

use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::{client, num_of, os_word, plain_text, proxy_line, send, send_json, text_of, Detail, Summary};

pub const CLOUD: &str = "https://app.octobrowser.net/api/v2/automation";
pub const LOCAL: &str = "http://localhost:58888";
const WHAT: &str = "Octo Browser";
const HINT: &str = "Copy it again from Octo Browser: Settings, API";
const PAGE: u64 = 100;
const FIELDS: &str = "title,description,proxy,start_pages,tags,has_user_password";

fn auth(r: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    r.header("X-Octo-Api-Token", token.trim())
}

/// `data` of a successful envelope; `success: false` is a refusal even on 200.
fn data_of(body: Value) -> Result<Value> {
    if body.get("success").and_then(Value::as_bool) == Some(false) {
        let why = text_of(body.get("msg"));
        bail!("{WHAT} refused: {}", if why.is_empty() { text_of(body.get("code")) } else { why });
    }
    Ok(body.get("data").cloned().unwrap_or(Value::Null))
}

async fn get(c: &reqwest::Client, url: &str, token: &str) -> Result<Value> {
    data_of(send_json(auth(c.get(url), token), WHAT, HINT).await?)
}

pub async fn list(cloud: &str, token: &str) -> Result<Vec<Summary>> {
    let c = client()?;
    let mut out = Vec::new();
    for page in 0..1000u64 {
        let body = send_json(
            auth(c.get(format!("{cloud}/profiles?page_len={PAGE}&page={page}&fields={FIELDS}")), token),
            WHAT,
            HINT,
        )
        .await?;
        let total = num_of(body.get("total_count"));
        let items = data_of(body)?.as_array().cloned().unwrap_or_default();
        for p in &items {
            let id = text_of(p.get("uuid"));
            if id.is_empty() {
                continue;
            }
            let proxy = p.get("proxy").filter(|x| x.is_object()).cloned().unwrap_or(Value::Null);
            let host = text_of(proxy.get("host"));
            out.push(Summary {
                id,
                name: text_of(p.get("title")),
                notes: plain_text(&text_of(p.get("description"))),
                tags: p
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|t| t.iter().map(|x| text_of(Some(x))).filter(|x| !x.is_empty()).collect())
                    .unwrap_or_default(),
                os: String::new(),
                proxy: (!host.is_empty()).then(|| format!("{host}:{}", num_of(proxy.get("port")))),
                extra: json!({ "locked": p.get("has_user_password").and_then(Value::as_bool).unwrap_or(false) }),
            });
        }
        if items.is_empty() || (page + 1) * PAGE >= total {
            break;
        }
    }
    Ok(out)
}

pub async fn profile(cloud: &str, local: &str, token: &str, summary: &Summary) -> Result<Detail> {
    let c = client()?;
    let p = get(&c, &format!("{cloud}/profiles/{}", summary.id), token).await?;

    let mut proxy = p.get("proxy").filter(|x| x.is_object()).cloned().unwrap_or(Value::Null);
    let saved = text_of(proxy.get("uuid"));
    if !saved.is_empty() && proxy.get("password").is_none() {
        // A saved proxy referenced without its credentials: the saved list has them.
        if let Ok(all) = get(&c, &format!("{cloud}/proxies"), token).await {
            if let Some(full) = all.as_array().and_then(|a| a.iter().find(|x| text_of(x.get("uuid")) == saved)) {
                proxy = full.clone();
            }
        }
    }
    let (proxy_line, proxy_note) = if !text_of(proxy.get("host")).is_empty() {
        proxy_line(
            &text_of(proxy.get("type")),
            &text_of(proxy.get("host")),
            num_of(proxy.get("port")),
            &text_of(proxy.get("login")),
            &text_of(proxy.get("password")),
        )
    } else {
        (None, None)
    };

    let start_url = p
        .get("start_pages")
        .and_then(Value::as_array)
        .and_then(|a| a.iter().map(|x| text_of(Some(x))).find(|u| !u.is_empty()));
    let os = Some(os_word(&text_of(p.pointer("/fingerprint/os")))).filter(|o| !o.is_empty());

    let locked = p.get("has_user_password").and_then(Value::as_bool).unwrap_or(false)
        || summary.extra.get("locked").and_then(Value::as_bool).unwrap_or(false);
    let (cookies, cookie_note) = if locked {
        (Vec::new(), Some("the profile has a password in Octo Browser, so its cookies were not read".to_string()))
    } else {
        match jar(local, &summary.id).await {
            Ok(cookies) if cookies.is_empty() => (cookies, Some("Octo Browser has no cookies in this profile".to_string())),
            Ok(cookies) => (cookies, None),
            Err(e) => (Vec::new(), Some(format!("cookies were not read: {e:#}"))),
        }
    };

    Ok(Detail { proxy_line, proxy_note, start_url, cookies, cookie_note, os })
}

/// The profile's cookies, read from the profile itself through the Octo app.
async fn jar(local: &str, uuid: &str) -> Result<Vec<Value>> {
    // Starting goes through the profile's proxy, and Octo's own start timeout
    // is set to two minutes below; the shared client gives up at one.
    let c = reqwest::Client::builder().timeout(Duration::from_secs(150)).build()?;
    let local_err = |e: anyhow::Error| {
        if format!("{e:#}").contains("did not answer") {
            anyhow::anyhow!("the Octo Browser app is not running on this computer (its local API is {local})")
        } else {
            e
        }
    };

    let (status, body) = send(
        c.post(format!("{local}/api/profiles/start")).json(&json!({
            "uuid": uuid, "headless": true, "debug_port": true, "only_local": true, "timeout": 120,
        })),
        WHAT,
        HINT,
    )
    .await
    .map_err(local_err)?;

    if status.is_success() {
        let ws = text_of(body.get("ws_endpoint"));
        let read = if ws.is_empty() {
            Err(anyhow::anyhow!("Octo Browser started the profile without a debugging port"))
        } else {
            crate::cookies::export(&ws).await
        };
        // Stopped whatever the read did: a headless profile left running holds
        // its lock in Octo's cloud and blocks the person from opening it.
        let _ = send(c.post(format!("{local}/api/profiles/stop")).json(&json!({ "uuid": uuid })), WHAT, HINT).await;
        return Ok(from_cdp(read?));
    }

    let code = text_of(body.get("code"));
    if code == "profiles.already_started" {
        // Open already: read it in place if Octo gave it a port, never close it.
        let (_, active) = send(c.get(format!("{local}/api/profiles/active")), WHAT, HINT).await.map_err(local_err)?;
        let ws = active
            .as_array()
            .and_then(|a| a.iter().find(|x| text_of(x.get("uuid")) == uuid))
            .map(|x| text_of(x.get("ws_endpoint")))
            .unwrap_or_default();
        if ws.is_empty() {
            bail!("the profile is open in Octo Browser; close it there and import it again");
        }
        return Ok(from_cdp(crate::cookies::export(&ws).await?));
    }
    let why = text_of(body.get("error"));
    bail!(
        "Octo Browser could not start the profile ({status}{})",
        if why.is_empty() { String::new() } else { format!(": {why}") }
    )
}

/// CDP's `Network.Cookie` into what `normalise_cookies` reads. CDP marks a
/// host-only cookie by a domain without the leading dot; `normalise_cookies`
/// wants it said, or the cookie would come back as a domain cookie sent to
/// every subdomain. Partitioned (CHIPS) cookies are left out: the import has
/// no partition to put them in, and unpartitioned they would be a different
/// cookie.
fn from_cdp(cookies: Vec<Value>) -> Vec<Value> {
    cookies
        .into_iter()
        .filter(|c| c.get("partitionKey").is_none_or(Value::is_null))
        .map(|mut c| {
            let host_only = !text_of(c.get("domain")).starts_with('.');
            if let Some(o) = c.as_object_mut() {
                o.insert("hostOnly".into(), host_only.into());
            }
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::{normalise_cookies, stand_in::serve};
    use super::*;

    #[tokio::test]
    async fn the_list_pages_through_and_reads_tags_notes_and_proxy() {
        let (base, seen) = serve(vec![
            ("GET", "/profiles?page_len=100&page=0&fields=title,description,proxy,start_pages,tags,has_user_password", 200,
             json!({"success": true, "msg": "", "code": null, "total_count": 2, "page": 0, "data": [
                {"uuid": "a1", "title": "Shop 1", "description": "FB <b>ads</b>", "tags": ["warm", "fb"],
                 "proxy": {"uuid": "", "type": "socks5", "host": "192.0.2.9", "port": 1080, "login": "u", "password": "p"},
                 "has_user_password": false},
                {"uuid": "a2", "title": "Shop 2", "description": "", "tags": [], "proxy": null, "has_user_password": true}]})),
        ])
        .await;
        let got = list(&base, "tok").await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].tags, vec!["warm", "fb"]);
        assert_eq!(got[0].notes, "FB ads");
        assert_eq!(got[0].proxy.as_deref(), Some("192.0.2.9:1080"));
        assert_eq!(got[1].proxy, None);
        assert_eq!(got[1].extra["locked"], true);
        assert!(seen.all()[0].to_lowercase().contains("x-octo-api-token: tok"));
        assert_eq!(seen.all().len(), 1, "two profiles of a total of two need one page");
    }

    #[tokio::test]
    async fn a_refusal_in_the_envelope_is_an_error() {
        let (base, _) = serve(vec![(
            "GET", "/profiles?page_len=100&page=0&fields=title,description,proxy,start_pages,tags,has_user_password", 200,
            json!({"success": false, "msg": "no subscription", "data": null, "code": "subscription"}),
        )])
        .await;
        let e = list(&base, "tok").await.unwrap_err();
        assert!(format!("{e:#}").contains("no subscription"));
    }

    fn summary(id: &str) -> Summary {
        Summary { id: id.into(), name: "x".into(), notes: String::new(), tags: vec![], os: String::new(), proxy: None,
                  extra: json!({"locked": false}) }
    }

    #[tokio::test]
    async fn a_saved_proxy_without_its_password_is_completed_and_the_os_and_start_page_come_over() {
        let (cloud, _) = serve(vec![
            ("GET", "/profiles/a1", 200, json!({"success": true, "data": {
                "uuid": "a1", "start_pages": ["", "https://example.com/"], "has_user_password": false,
                "proxy": {"uuid": "px", "type": "http", "host": "198.51.100.4", "port": 8000, "title": "de"},
                "fingerprint": {"os": "mac"}}})),
            ("GET", "/proxies", 200, json!({"success": true, "data": [
                {"uuid": "px", "type": "http", "host": "198.51.100.4", "port": 8000, "login": "u", "password": "p@ss"}]})),
        ])
        .await;
        // Nothing listens on the local side: the import goes on without cookies.
        let (local, _) = serve(vec![]).await;
        let d = profile(&cloud, &local, "tok", &summary("a1")).await.unwrap();
        assert_eq!(d.proxy_line.as_deref(), Some("http://u:p@ss@198.51.100.4:8000"));
        assert_eq!(d.start_url.as_deref(), Some("https://example.com/"));
        assert_eq!(d.os.as_deref(), Some("mac"));
        assert!(d.cookies.is_empty());
        assert!(d.cookie_note.unwrap().contains("could not start"));
    }

    #[tokio::test]
    async fn a_locked_profile_is_not_started() {
        let (cloud, _) = serve(vec![("GET", "/profiles/a2", 200, json!({"success": true, "data": {
            "uuid": "a2", "has_user_password": true, "proxy": null, "fingerprint": {"os": "win"}}}))])
        .await;
        let (local, seen) = serve(vec![]).await;
        let d = profile(&cloud, &local, "tok", &summary("a2")).await.unwrap();
        assert!(d.cookie_note.unwrap().contains("password"));
        assert!(seen.all().is_empty(), "nothing was sent to the Octo app");
    }

    #[tokio::test]
    async fn an_open_profile_without_a_port_says_to_close_it() {
        let (cloud, _) = serve(vec![("GET", "/profiles/a1", 200, json!({"success": true, "data": {
            "uuid": "a1", "proxy": null, "fingerprint": {"os": "win"}}}))])
        .await;
        let (local, _) = serve(vec![
            ("POST", "/api/profiles/start", 400, json!({"code": "profiles.already_started", "error": "Profile is already started"})),
            ("GET", "/api/profiles/active", 200, json!([{"uuid": "a1", "state": "STARTED", "headless": false, "ws_endpoint": ""}])),
        ])
        .await;
        let d = profile(&cloud, &local, "tok", &summary("a1")).await.unwrap();
        assert!(d.cookie_note.unwrap().contains("close it there"));
    }

    #[tokio::test]
    async fn no_octo_app_is_said_in_words() {
        let e = jar("http://127.0.0.1:9", "a1").await.unwrap_err();
        assert!(format!("{e:#}").contains("not running"), "{e:#}");
    }

    #[test]
    fn cdp_cookies_keep_host_only_and_drop_partitioned_ones() {
        let got = normalise_cookies(&from_cdp(vec![
            json!({"name": "sid", "value": "1", "domain": "www.example.com", "path": "/", "expires": -1, "session": true,
                   "secure": true, "httpOnly": true, "sameSite": "Lax"}),
            json!({"name": "datr", "value": "2", "domain": ".facebook.com", "path": "/", "expires": 1893456000.5,
                   "session": false, "secure": true, "httpOnly": true, "sameSite": "None"}),
            json!({"name": "chips", "value": "3", "domain": ".x.com", "path": "/", "expires": -1, "session": true,
                   "partitionKey": {"topLevelSite": "https://y.com", "hasCrossSiteAncestor": true}}),
        ]));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["url"], "https://www.example.com/");
        assert!(got[0].get("expires").is_none(), "a session cookie has no expiry");
        assert_eq!(got[1]["domain"], ".facebook.com");
    }
}
