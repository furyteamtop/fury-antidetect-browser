// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Kameleo, through its Local API (developer.kameleo.io/static/swagger.json,
//! 5.3.1). The app has to be running; there is no token on the HTTP API.
//!
//! - `GET /profiles` is a bare array, no pages. Notes and the start page are
//!   only in `GET /profiles/{id}`, which the list reads for each profile.
//! - The proxy is inline: `{value: none|http|socks5|ssh, extra: {host, port,
//!   id, secret}}`, `id` the username and `secret` the password.
//! - Cookies refuse a running profile with 409; that profile comes over
//!   without them, and `cookie_note` says to close it first. Kameleo never
//!   returns session cookies.
//! - Errors are RFC 7807 `{status, errorCode, title}`.

use anyhow::Result;
use serde_json::{json, Value};

use super::{client, num_of, os_word, plain_text, proxy_line, send, send_json, text_of, Detail, Summary};

pub const BASE: &str = "http://localhost:5050";
const WHAT: &str = "Kameleo";
const HINT: &str = "Kameleo's Local API takes no token; check that the app is running and signed in";

pub async fn list(base: &str) -> Result<Vec<Summary>> {
    let c = client()?;
    let body = send_json(c.get(format!("{base}/profiles")), WHAT, HINT).await?;
    let items = body.as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for p in &items {
        let id = text_of(p.get("id"));
        if id.is_empty() {
            continue;
        }
        let detail = send_json(c.get(format!("{base}/profiles/{id}")), WHAT, HINT).await.unwrap_or(Value::Null);
        let proxy = detail.get("proxy").filter(|x| x.is_object()).or_else(|| p.get("proxy")).cloned().unwrap_or(Value::Null);
        let kind = text_of(proxy.get("value"));
        let host = text_of(proxy.pointer("/extra/host"));
        let start = text_of(detail.get("startPage"));
        out.push(Summary {
            id,
            name: text_of(p.get("name")),
            notes: plain_text(&text_of(detail.get("notes"))),
            tags: p
                .get("tags")
                .and_then(Value::as_array)
                .map(|t| t.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
            os: os_word(&text_of(p.pointer("/os/family"))),
            proxy: (kind != "none" && !host.is_empty())
                .then(|| format!("{host}:{}", num_of(proxy.pointer("/extra/port")))),
            extra: json!({
                "proxy": proxy,
                "start_url": if start.is_empty() { Value::Null } else { start.into() },
                "running": text_of(p.pointer("/status/lifetimeState")),
            }),
        });
    }
    Ok(out)
}

pub async fn profile(base: &str, summary: &Summary) -> Result<Detail> {
    let c = client()?;
    let proxy = summary.extra.get("proxy").cloned().unwrap_or(Value::Null);
    let kind = text_of(proxy.get("value"));
    let (proxy_line, proxy_note) = if kind.is_empty() || kind == "none" {
        (None, None)
    } else {
        proxy_line(
            &kind,
            &text_of(proxy.pointer("/extra/host")),
            num_of(proxy.pointer("/extra/port")),
            &text_of(proxy.pointer("/extra/id")),
            &text_of(proxy.pointer("/extra/secret")),
        )
    };

    let (status, body) = send(c.get(format!("{base}/profiles/{}/cookies", summary.id)), WHAT, HINT).await?;
    let (cookies, cookie_note) = if status.is_success() {
        let cookies = body.as_array().cloned().unwrap_or_default();
        let note = cookies.is_empty().then(|| "Kameleo has no cookies stored for this profile".to_string());
        (cookies, note)
    } else if status == reqwest::StatusCode::CONFLICT {
        (Vec::new(), Some("the profile is running in Kameleo, which gives no cookies of a running profile; close it and import it again".into()))
    } else {
        (
            Vec::new(),
            Some(format!(
                "Kameleo answered {status} for the cookies{}",
                super::error_text(&body).map(|t| format!(": {t}")).unwrap_or_default()
            )),
        )
    };
    let start_url = summary.extra.get("start_url").and_then(Value::as_str).map(str::to_string);
    Ok(Detail { proxy_line, proxy_note, start_url, cookies, cookie_note })
}

#[cfg(test)]
mod tests {
    use super::super::stand_in::serve;
    use super::*;

    #[tokio::test]
    async fn the_list_brings_notes_and_start_page_from_the_detail() {
        let (base, _) = serve(vec![
            ("GET", "/profiles", 200, json!([{"id": "a7ca", "name": "mystic-turtle", "tags": ["facebook"],
                "proxy": {"value": "socks5", "extra": {"host": "127.0.0.1", "port": 9951, "id": "username", "secret": "password"}},
                "os": {"family": "macos", "version": "15", "platform": "64"},
                "status": {"lifetimeState": "terminated"}}])),
            ("GET", "/profiles/a7ca", 200, json!({"id": "a7ca", "notes": "ad verification",
                "startPage": "https://whoer.net/",
                "proxy": {"value": "socks5", "extra": {"host": "127.0.0.1", "port": 9951, "id": "username", "secret": "password"}}})),
        ])
        .await;
        let got = list(&base).await.unwrap();
        assert_eq!(got[0].os, "mac");
        assert_eq!(got[0].notes, "ad verification");
        assert_eq!(got[0].proxy.as_deref(), Some("127.0.0.1:9951"));
        assert_eq!(got[0].extra["start_url"], "https://whoer.net/");
    }

    #[tokio::test]
    async fn a_running_profile_comes_over_without_cookies_and_says_why() {
        let (base, _) = serve(vec![("GET", "/profiles/a7ca/cookies", 409,
            json!({"status": 409, "errorCode": "profile_running", "title": "Profile must be terminated."}))]).await;
        let s = Summary { id: "a7ca".into(), name: "x".into(), notes: String::new(), tags: vec![], os: String::new(),
            proxy: None, extra: json!({"proxy": {"value": "socks5", "extra": {"host": "h", "port": 1080, "id": "u", "secret": "s"}},
                                       "start_url": "https://whoer.net/"}) };
        let d = profile(&base, &s).await.unwrap();
        assert_eq!(d.proxy_line.as_deref(), Some("socks5://u:s@h:1080"));
        assert!(d.cookies.is_empty());
        assert!(d.cookie_note.unwrap().contains("close it"));
        assert_eq!(d.start_url.as_deref(), Some("https://whoer.net/"));
    }

    #[tokio::test]
    async fn a_stopped_profile_brings_its_cookies() {
        let (base, _) = serve(vec![("GET", "/profiles/a7ca/cookies", 200, json!([
            {"domain": ".google.com", "name": "_ga", "path": "/", "value": "GA1", "hostOnly": false,
             "httpOnly": true, "secure": true, "sameSite": "unspecified", "expirationDate": 1893456000,
             "session": false, "storeId": "0"}]))]).await;
        let s = Summary { id: "a7ca".into(), name: "x".into(), notes: String::new(), tags: vec![], os: String::new(),
            proxy: None, extra: json!({"proxy": {"value": "none", "extra": null}}) };
        let d = profile(&base, &s).await.unwrap();
        assert_eq!(d.proxy_line, None);
        assert_eq!(d.cookies.len(), 1);
    }
}
