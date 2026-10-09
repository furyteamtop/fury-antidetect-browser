// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Vision, through its cloud API (docs.browser.vision). The token is the
//! app's X-Token (Settings, X-Token, Copy); in team mode the app shows an
//! X-Team-Token instead, so the header is tried as X-Token and then as
//! X-Team-Token, and the one that worked is kept for the profile calls.
//!
//! - Everything lives in folders: the list walks `GET /folders`, then each
//!   folder's profiles (`pn` from 0, `ps`), tags and proxies.
//! - Tags on a profile are tag ids; proxies are referenced by `proxy_id`.
//!   Both are resolved from the folder's own lists.
//! - Notes may be rich text, and come over as plain text.
//! - Start pages are set per folder in the app and are not in the API.
//! - The cookie export documents name, value, path, domain and expires only;
//!   secure, http_only and same_site are read when present.

use std::collections::HashMap;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::{client, num_of, os_word, plain_text, proxy_line, send, send_json, text_of, Detail, Summary};

pub const BASE: &str = "https://api.browser.vision/api/v1";
const WHAT: &str = "Vision";
const HINT: &str = "Copy it again from Vision: Settings, X-Token";

fn auth(r: reqwest::RequestBuilder, header: &str, token: &str) -> reqwest::RequestBuilder {
    r.header(header, token.trim())
}

/// GET with whichever header this token belongs under.
async fn get(c: &reqwest::Client, url: &str, header: &str, token: &str) -> Result<Value> {
    send_json(auth(c.get(url), header, token), WHAT, HINT).await
}

pub async fn list(base: &str, token: &str) -> Result<Vec<Summary>> {
    let c = client()?;

    // Which header: X-Token for a personal token, X-Team-Token for a team's.
    let mut header = "X-Token";
    let (status, mut folders) = match send(auth(c.get(format!("{base}/folders")), "X-Token", token), WHAT, HINT).await {
        Ok(r) => r,
        Err(_) => {
            header = "X-Team-Token";
            send(auth(c.get(format!("{base}/folders")), header, token), WHAT, HINT).await?
        }
    };
    if !status.is_success() {
        header = "X-Team-Token";
        let (s2, f2) = send(auth(c.get(format!("{base}/folders")), header, token), WHAT, HINT).await?;
        if !s2.is_success() {
            bail!("{WHAT} answered {s2} for its folders");
        }
        folders = f2;
    }

    let mut out = Vec::new();
    for folder in folders.get("data").and_then(Value::as_array).cloned().unwrap_or_default() {
        let fid = text_of(folder.get("id"));
        if fid.is_empty() {
            continue;
        }
        let tags: HashMap<String, String> = get(&c, &format!("{base}/folders/{fid}/tags"), header, token)
            .await
            .ok()
            .and_then(|b| b.get("data").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .iter()
            .map(|t| (text_of(t.get("id")), text_of(t.get("tag_name"))))
            .collect();
        let proxies: HashMap<String, Value> = get(&c, &format!("{base}/folders/{fid}/proxies"), header, token)
            .await
            .ok()
            .and_then(|b| b.get("data").and_then(Value::as_array).cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|p| (text_of(p.get("id")), p))
            .collect();

        const PS: u64 = 100;
        for pn in 0..1000u64 {
            let body = get(&c, &format!("{base}/folders/{fid}/profiles?pn={pn}&ps={PS}"), header, token).await?;
            let items = body.pointer("/data/items").and_then(Value::as_array).cloned().unwrap_or_default();
            let total = num_of(body.pointer("/data/total"));
            for p in &items {
                let id = text_of(p.get("id"));
                if id.is_empty() {
                    continue;
                }
                let proxy = proxies
                    .get(&text_of(p.get("proxy_id")))
                    .cloned()
                    .or_else(|| p.get("proxy").filter(|x| x.is_object()).cloned())
                    .unwrap_or(Value::Null);
                let host = text_of(proxy.get("proxy_ip"));
                out.push(Summary {
                    id,
                    name: text_of(p.get("profile_name")),
                    notes: plain_text(&text_of(p.get("profile_notes"))),
                    tags: p
                        .get("profile_tags")
                        .and_then(Value::as_array)
                        .map(|ids| ids.iter().filter_map(|t| tags.get(&text_of(Some(t))).cloned()).collect())
                        .unwrap_or_default(),
                    os: os_word(&text_of(p.get("platform"))),
                    proxy: (!host.is_empty()).then(|| format!("{host}:{}", num_of(proxy.get("proxy_port")))),
                    extra: json!({ "folder_id": fid, "header": header, "proxy": proxy }),
                });
            }
            if items.is_empty() || (pn + 1) * PS >= total {
                break;
            }
        }
    }
    Ok(out)
}

pub async fn profile(base: &str, token: &str, summary: &Summary) -> Result<Detail> {
    let c = client()?;
    let fid = text_of(summary.extra.get("folder_id"));
    if fid.is_empty() {
        bail!("this Vision profile came without its folder; list the profiles again");
    }
    let header = match text_of(summary.extra.get("header")).as_str() {
        "X-Team-Token" => "X-Team-Token",
        _ => "X-Token",
    };
    let proxy = summary.extra.get("proxy").cloned().unwrap_or(Value::Null);
    let (proxy_line, proxy_note) = if proxy.is_object() && !text_of(proxy.get("proxy_ip")).is_empty() {
        proxy_line(
            &text_of(proxy.get("proxy_type")),
            &text_of(proxy.get("proxy_ip")),
            num_of(proxy.get("proxy_port")),
            &text_of(proxy.get("proxy_username")),
            &text_of(proxy.get("proxy_password")),
        )
    } else {
        (None, None)
    };
    let body = get(&c, &format!("{base}/cookies/{fid}/{}", summary.id), header, token).await?;
    let cookies = body.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
    let cookie_note = cookies.is_empty().then(|| "Vision has no cookies stored for this profile".to_string());
    Ok(Detail { proxy_line, proxy_note, start_url: None, cookies, cookie_note })
}

#[cfg(test)]
mod tests {
    use super::super::stand_in::serve;
    use super::*;

    #[tokio::test]
    async fn folders_tags_and_proxies_are_resolved_into_each_profile() {
        let (base, seen) = serve(vec![
            ("GET", "/folders", 200, json!({"data": [{"id": "f1", "folder_name": "Google"}]})),
            ("GET", "/folders/f1/tags", 200, json!({"data": [{"id": "t1", "tag_name": "warm"}]})),
            ("GET", "/folders/f1/proxies", 200, json!({"data": [{"id": "px1", "proxy_type": "SOCKS5",
                "proxy_ip": "192.0.2.9", "proxy_port": 12345, "proxy_username": "u", "proxy_password": "p"}]})),
            ("GET", "/folders/f1/profiles?pn=0&ps=100", 200, json!({"data": {"total": "1", "items": [
                {"id": "p1", "folder_id": "f1", "proxy_id": "px1", "profile_name": "Matthew",
                 "profile_notes": "<p>FB <b>login</b></p>", "profile_tags": ["t1"], "platform": "macos", "proxy": null}]}})),
        ])
        .await;
        let got = list(&base, "tok").await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tags, vec!["warm"]);
        assert_eq!(got[0].notes, "FB login");
        assert_eq!(got[0].os, "mac");
        assert_eq!(got[0].proxy.as_deref(), Some("192.0.2.9:12345"));
        assert!(seen.all()[0].to_lowercase().contains("x-token: tok"));
    }

    #[tokio::test]
    async fn a_profile_brings_its_proxy_and_cookies() {
        let (base, _) = serve(vec![("GET", "/cookies/f1/p1", 200, json!({"data": [
            {"name": "datr", "value": "52", "path": "/", "domain": ".facebook.com", "expires": 1893456000}]}))]).await;
        let s = Summary { id: "p1".into(), name: "x".into(), notes: String::new(), tags: vec![], os: String::new(), proxy: None,
            extra: json!({"folder_id": "f1", "header": "X-Token", "proxy": {"proxy_type": "SOCKS5", "proxy_ip": "192.0.2.9",
                          "proxy_port": 12345, "proxy_username": "u", "proxy_password": "p"}}) };
        let d = profile(&base, "tok", &s).await.unwrap();
        assert_eq!(d.proxy_line.as_deref(), Some("socks5://u:p@192.0.2.9:12345"));
        assert_eq!(d.cookies.len(), 1);
    }
}
