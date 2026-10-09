// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Dolphin{anty}, through its cloud API (OpenAPI "Dolphin{anty} Public API"
//! v1.0.7, docs.dolphin-anty-cdn.com). Token: dolphin-anty.com/panel, API.
//!
//! Three hosts, by endpoint, as the specification assigns them:
//! profiles and proxies on dolphin-anty-api.com, homepages (start pages) on
//! apiv2, cookies on darkwing, Dolphin's sync service.
//!
//! - The list is `GET /browser_profiles/list-cursor` with
//!   `with_proxy_credentials=true`, which puts the proxy's port, login and
//!   password in each item. The page-numbered `GET /browser_profiles` is
//!   switched off on 1 November 2026 (410), and the cursor route answers 404
//!   on accounts where it is not enabled yet, so the cursor route is tried
//!   first and the old one taken only on its 404.
//! - Start pages are homepage ids on the profile, resolved through
//!   `GET /homepages` once per import.
//! - Cookies come from `POST darkwing/cookies/export`, the cloud copy, which
//!   needs no Dolphin app running; a profile with cloud sync off answers 400
//!   and comes over without cookies, said in `cookie_note`. The expiry is in
//!   milliseconds by the schema and in seconds in its own example, which
//!   `normalise_cookies` settles by magnitude.

use std::collections::HashMap;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::{client, num_of, os_word, plain_text, proxy_line, send, send_json, text_of, Detail, Summary};

const WHAT: &str = "Dolphin";
const HINT: &str = "Make a new one at dolphin-anty.com/panel, API";

pub struct Hosts {
    pub api: String,
    pub v2: String,
    pub darkwing: String,
}

impl Hosts {
    /// The real hosts, or all three under one stand-in base for tests.
    pub fn for_base(base: Option<&str>) -> Self {
        match base.map(str::trim).filter(|b| !b.is_empty()) {
            Some(b) => {
                let b = b.trim_end_matches('/');
                Hosts { api: b.into(), v2: format!("{b}/api/v2"), darkwing: format!("{b}/api/v1") }
            }
            None => Hosts {
                api: "https://dolphin-anty-api.com".into(),
                v2: "https://apiv2.dolphin-anty-api.com/api/v2".into(),
                darkwing: "https://darkwing.dolphin-anty-api.com/api/v1".into(),
            },
        }
    }
}

pub async fn list(hosts: &Hosts, token: &str) -> Result<Vec<Summary>> {
    let c = client()?;
    let mut items: Vec<Value> = Vec::new();

    // The cursor route first.
    let mut cursor: Option<String> = None;
    let mut cursor_ok = true;
    for _ in 0..1000 {
        let mut url = format!("{}/browser_profiles/list-cursor?limit=100&with_proxy_credentials=true", hosts.api);
        if let Some(cur) = &cursor {
            url.push_str(&format!("&cursor={}", urlencode(cur)));
        }
        let (status, body) = send(c.get(&url).bearer_auth(token), WHAT, HINT).await?;
        if status == reqwest::StatusCode::NOT_FOUND && cursor.is_none() {
            cursor_ok = false;
            break;
        }
        if !status.is_success() {
            bail!("{WHAT} answered {status} for its profile list");
        }
        items.extend(body.get("data").and_then(Value::as_array).cloned().unwrap_or_default());
        match body.get("next_page_url").and_then(Value::as_str).and_then(cursor_of) {
            Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
            _ => break,
        }
    }

    // The page-numbered route, while it still exists.
    if !cursor_ok {
        for page in 1..=1000u32 {
            let url = format!("{}/browser_profiles?limit=100&page={page}", hosts.api);
            let (status, body) = send(c.get(&url).bearer_auth(token), WHAT, HINT).await?;
            if status == reqwest::StatusCode::GONE {
                bail!(
                    "{WHAT} switched its old profile list off and has not enabled the new one \
                     for this account yet; its support enables cursor pagination on request"
                );
            }
            if !status.is_success() {
                bail!("{WHAT} answered {status} for its profile list");
            }
            let page_items = body.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
            let done = page_items.is_empty() || body.get("next_page_url").map_or(true, Value::is_null);
            items.extend(page_items);
            if done {
                break;
            }
        }
    }

    // Start pages: one walk through the account's homepages, not one per
    // profile. Not having them costs a start page, not the import.
    let wanted = items.iter().any(|p| {
        p.get("homepages").and_then(Value::as_array).is_some_and(|h| !h.is_empty())
    });
    let homepages = if wanted { homepages(&c, hosts, token).await.unwrap_or_default() } else { HashMap::new() };

    Ok(items.iter().filter_map(|p| summary_of(p, &homepages)).collect())
}

async fn homepages(c: &reqwest::Client, hosts: &Hosts, token: &str) -> Result<HashMap<u64, String>> {
    let mut out = HashMap::new();
    for page in 1..=100u32 {
        let url = format!("{}/homepages?limit=100&page={page}", hosts.v2);
        let body = send_json(c.get(&url).bearer_auth(token), WHAT, HINT).await?;
        let data = body.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
        for h in &data {
            let url = text_of(h.get("url"));
            if !url.is_empty() {
                out.insert(num_of(h.get("id")), url);
            }
        }
        if data.is_empty() || body.get("next_page_url").map_or(true, Value::is_null) {
            break;
        }
    }
    Ok(out)
}

/// The `cursor` parameter out of a next_page_url.
fn cursor_of(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url)
        .or_else(|_| reqwest::Url::parse(&format!("http://x{url}")))
        .ok()?;
    let found = parsed.query_pairs().find(|(k, _)| k == "cursor").map(|(_, v)| v.into_owned());
    found
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Notes arrive as a string, a rich-text object (its `content`), an array of
/// those, or null, depending on the endpoint's age; all are read.
pub(crate) fn notes_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => plain_text(s),
        Some(Value::Object(o)) => o.get("content").and_then(Value::as_str).map(plain_text).unwrap_or_default(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|n| notes_of(Some(n)))
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub(crate) fn summary_of(p: &Value, homepages: &HashMap<u64, String>) -> Option<Summary> {
    let id = text_of(p.get("id"));
    if id.is_empty() {
        return None;
    }
    let proxy = p.get("proxy").filter(|x| x.is_object());
    let host = proxy.map(|x| text_of(x.get("host"))).unwrap_or_default();
    let port = proxy.map(|x| num_of(x.get("port"))).unwrap_or(0);
    let start_url = p
        .get("homepages")
        .and_then(Value::as_array)
        .and_then(|h| {
            let mut ordered: Vec<&Value> = h.iter().collect();
            ordered.sort_by_key(|x| num_of(x.get("order")));
            ordered.into_iter().find_map(|x| homepages.get(&num_of(x.get("id"))).cloned())
        });
    Some(Summary {
        id,
        name: text_of(p.get("name")),
        notes: notes_of(p.get("notes")),
        tags: p
            .get("tags")
            .and_then(Value::as_array)
            .map(|t| t.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        os: os_word(&text_of(p.get("platform"))),
        proxy: (!host.is_empty()).then(|| if port > 0 { format!("{host}:{port}") } else { host.clone() }),
        extra: json!({
            "proxy": proxy.cloned().unwrap_or(Value::Null),
            "start_url": start_url,
            "requirePassword": p.get("requirePassword").and_then(Value::as_bool).unwrap_or(false),
        }),
    })
}

pub async fn profile(hosts: &Hosts, token: &str, summary: &Summary) -> Result<Detail> {
    let c = client()?;
    let id: u64 = summary.id.parse().map_err(|_| anyhow::anyhow!("{:?} is not a Dolphin profile id", summary.id))?;

    // The proxy, with its credentials. The cursor list carries them; the old
    // list does not, and then the proxy list by id does.
    let mut proxy = summary.extra.get("proxy").cloned().unwrap_or(Value::Null);
    if proxy.is_object() && num_of(proxy.get("port")) == 0 {
        let pid = text_of(proxy.get("id"));
        if !pid.is_empty() {
            let url = format!("{}/proxy?ids={pid}", hosts.api);
            if let Ok(body) = send_json(c.get(&url).bearer_auth(token), WHAT, HINT).await {
                if let Some(full) = body.get("data").and_then(Value::as_array).and_then(|d| d.first()) {
                    proxy = full.clone();
                }
            }
        }
    }
    let (proxy_line, proxy_note) = if proxy.is_object() {
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

    // The cookies, from Dolphin's cloud copy.
    let mut cookie_note = None;
    let mut cookies = Vec::new();
    if summary.extra.get("requirePassword").and_then(Value::as_bool).unwrap_or(false) {
        cookie_note = Some("the profile is password-protected in Dolphin, so its cookies were not read".into());
    } else {
        let (status, body) = send(
            c.post(format!("{}/cookies/export", hosts.darkwing))
                .bearer_auth(token)
                .json(&json!({ "browserProfileId": id, "browserProfilePassword": null })),
            WHAT,
            HINT,
        )
        .await?;
        if status.is_success() {
            cookies = body.get("cookies").and_then(Value::as_array).cloned().unwrap_or_default();
            if cookies.is_empty() {
                cookie_note = Some("Dolphin's cloud copy of this profile has no cookies".into());
            }
        } else if status == reqwest::StatusCode::BAD_REQUEST {
            cookie_note = Some("cloud sync is off for this profile in Dolphin, so it has no cookies to export".into());
        } else {
            cookie_note = Some(format!(
                "Dolphin answered {status} for the cookies{}",
                super::error_text(&body).map(|t| format!(": {t}")).unwrap_or_default()
            ));
        }
    }

    let start_url = summary.extra.get("start_url").and_then(Value::as_str).map(str::to_string);
    Ok(Detail { proxy_line, proxy_note, start_url, cookies, cookie_note })
}

#[cfg(test)]
mod tests {
    use super::super::stand_in::serve;
    use super::*;

    fn item(id: u64, name: &str) -> Value {
        json!({"id": id, "name": name, "platform": "windows", "browserType": "anty",
               "tags": ["work"], "homepages": [{"id": 77, "name": "Mail", "order": 1}],
               "notes": {"content": "<p>login: <b>x</b> &amp; y</p>", "color": "blue"},
               "proxy": {"id": 123, "type": "http", "host": "proxy.example.com",
                         "port": 8080, "login": "u", "password": "p:w"}})
    }

    #[tokio::test]
    async fn the_cursor_list_is_followed_and_start_pages_resolved() {
        let (base, seen) = serve(vec![
            ("GET", "/browser_profiles/list-cursor?limit=100&with_proxy_credentials=true", 200,
             json!({"data": [item(1, "A")], "next_page_url": "https://dolphin-anty-api.com/browser_profiles/list-cursor?limit=100&cursor=abc%3D%3D"})),
            ("GET", "/browser_profiles/list-cursor?limit=100&with_proxy_credentials=true&cursor=abc%3D%3D", 200,
             json!({"data": [item(2, "B")], "next_page_url": null})),
            ("GET", "/api/v2/homepages?limit=100&page=1", 200,
             json!({"data": [{"id": 77, "name": "Mail", "url": "https://mail.example"}], "next_page_url": null})),
        ])
        .await;
        let got = list(&Hosts::for_base(Some(&base)), "tok").await.unwrap();
        assert_eq!(got.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(got[0].id, "1");
        assert_eq!(got[0].os, "win");
        assert_eq!(got[0].notes, "login: x & y");
        assert_eq!(got[0].proxy.as_deref(), Some("proxy.example.com:8080"));
        assert_eq!(got[0].extra["start_url"], "https://mail.example");
        assert!(seen.all()[0].to_lowercase().contains("authorization: bearer tok"));
    }

    #[tokio::test]
    async fn a_404_on_the_cursor_route_falls_back_to_pages() {
        let (base, _) = serve(vec![
            ("GET", "/browser_profiles/list-cursor?limit=100&with_proxy_credentials=true", 404,
             json!({"message": "Cursor pagination is not enabled for your account."})),
            ("GET", "/browser_profiles?limit=100&page=1", 200,
             json!({"data": [{"id": 5, "name": "Old", "platform": "macos",
                              "proxy": {"id": 9, "type": "socks5", "host": "h"}}],
                    "next_page_url": null})),
        ])
        .await;
        let got = list(&Hosts::for_base(Some(&base)), "t").await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].os, "mac");
        assert_eq!(got[0].proxy.as_deref(), Some("h"));
    }

    #[tokio::test]
    async fn neither_list_working_is_said_plainly() {
        let (base, _) = serve(vec![
            ("GET", "/browser_profiles/list-cursor?limit=100&with_proxy_credentials=true", 404, json!({})),
            ("GET", "/browser_profiles?limit=100&page=1", 410, json!({"success": false})),
        ])
        .await;
        let e = format!("{:#}", list(&Hosts::for_base(Some(&base)), "t").await.unwrap_err());
        assert!(e.contains("cursor pagination"), "{e}");
    }

    #[tokio::test]
    async fn a_profile_brings_its_proxy_and_cloud_cookies() {
        let (base, seen) = serve(vec![
            ("POST", "/api/v1/cookies/export", 200, json!({"success": true, "cookies": [
                {"name": "sid", "value": "v", "domain": ".example.com", "path": "/",
                 "expirationDate": 1893456000000u64, "secure": true, "httpOnly": true, "sameSite": "no_restriction"}]})),
        ])
        .await;
        let s = summary_of(&item(42, "A"), &HashMap::new()).unwrap();
        let d = profile(&Hosts::for_base(Some(&base)), "t", &s).await.unwrap();
        assert_eq!(d.proxy_line.as_deref(), Some("http://u:p:w@proxy.example.com:8080"));
        assert_eq!(d.cookies.len(), 1);
        assert!(seen.all()[0].contains(r#""browserProfileId":42"#));
    }

    #[tokio::test]
    async fn the_old_lists_proxy_is_completed_from_the_proxy_list() {
        let (base, _) = serve(vec![
            ("GET", "/proxy?ids=9", 200, json!({"data": [{"id": 9, "type": "socks5", "host": "h",
                "port": 1080, "login": "l", "password": "pw"}]})),
            ("POST", "/api/v1/cookies/export", 400, json!({"message": "sync disabled"})),
        ])
        .await;
        let s = summary_of(&json!({"id": 5, "name": "Old", "platform": "macos",
                                   "proxy": {"id": 9, "type": "socks5", "host": "h"}}), &HashMap::new()).unwrap();
        let d = profile(&Hosts::for_base(Some(&base)), "t", &s).await.unwrap();
        assert_eq!(d.proxy_line.as_deref(), Some("socks5://l:pw@h:1080"));
        assert!(d.cookies.is_empty());
        assert!(d.cookie_note.unwrap().contains("cloud sync is off"));
    }

    #[test]
    fn notes_in_every_shape_become_text() {
        assert_eq!(notes_of(Some(&json!("plain"))), "plain");
        assert_eq!(notes_of(Some(&json!([{"content": "<p>a</p>"}, {"content": "b"}]))), "a\nb");
        assert_eq!(notes_of(Some(&Value::Null)), "");
    }
}
