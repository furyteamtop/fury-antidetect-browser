// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Undetectable, through its Local API ("Undetectable Local API" 1.5.0).
//! The app has to be running and signed in, on a paid plan; there is no
//! token.
//!
//! - Every answer is `{code, status, data}`; `code` other than 0 is the
//!   failure, with `data.error` saying why.
//! - `GET /list` is an object keyed by profile id, Chromium profiles only,
//!   and carries no notes, OS or proxy: `GET /profile/getinfo/{id}` does,
//!   so the list reads each profile's detail. Local, so that is cheap.
//! - The proxy is one string, `socks5://host:port:login:password`, not the
//!   URL form, or the id of a saved proxy, or "none".
//! - Cookies spell sameSite `samesite`, with "Unspecefied" (sic);
//!   `normalise_cookies` reads that.

use std::collections::HashMap;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::{client, num_of, os_word, plain_text, proxy_line, send_json, text_of, Detail, Summary};

pub const BASE: &str = "http://127.0.0.1:25325";
const WHAT: &str = "Undetectable";
const HINT: &str = "Undetectable's Local API takes no token; check that the app is signed in";

fn data_of(body: Value) -> Result<Value> {
    if body.get("code").and_then(Value::as_i64) != Some(0) {
        let why = text_of(body.pointer("/data/error"));
        bail!("{WHAT} refused: {}", if why.is_empty() { "no reason given".into() } else { why });
    }
    Ok(body.get("data").cloned().unwrap_or(Value::Null))
}

pub async fn list(base: &str) -> Result<Vec<Summary>> {
    let c = client()?;
    let data = data_of(send_json(c.get(format!("{base}/list")), WHAT, HINT).await?)?;
    let Some(map) = data.as_object() else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for (id, item) in map {
        // The detail has what the list lacks; a profile whose detail fails
        // still comes over, by its list entry.
        let detail = match send_json(c.get(format!("{base}/profile/getinfo/{id}")), WHAT, HINT).await {
            Ok(body) => data_of(body).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
        let pick = |k: &str| detail.get(k).or_else(|| item.get(k));
        let proxy = text_of(pick("proxy"));
        let tags = pick("tags")
            .and_then(Value::as_array)
            .map(|t| t.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        out.push(Summary {
            id: id.clone(),
            name: text_of(pick("name")),
            notes: plain_text(&text_of(pick("notes"))),
            tags,
            os: os_word(&text_of(pick("os"))),
            proxy: shown(&proxy),
            extra: json!({ "proxy": proxy }),
        });
    }
    Ok(out)
}

fn shown(proxy: &str) -> Option<String> {
    let p = proxy.trim();
    if p.is_empty() || p.eq_ignore_ascii_case("none") {
        return None;
    }
    match parse(p) {
        Some((_, host, port, _, _)) => Some(format!("{host}:{port}")),
        None => Some("saved proxy".into()),
    }
}

/// `scheme://host:port[:login:password]`, the password allowed to contain
/// colons. None for anything else, which is a saved proxy's id.
pub(crate) fn parse(p: &str) -> Option<(String, String, u64, String, String)> {
    let (scheme, rest) = p.split_once("://")?;
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let port: u64 = parts[1].trim().parse().ok()?;
    let user = parts.get(2).copied().unwrap_or("").to_string();
    let pass = if parts.len() > 3 { parts[3..].join(":") } else { String::new() };
    Some((scheme.to_string(), parts[0].to_string(), port, user, pass))
}

pub async fn profile(base: &str, summary: &Summary) -> Result<Detail> {
    let c = client()?;
    let raw = text_of(summary.extra.get("proxy"));
    let (proxy_line, proxy_note) = if raw.is_empty() || raw.eq_ignore_ascii_case("none") {
        (None, None)
    } else if let Some((kind, host, port, user, pass)) = parse(&raw) {
        proxy_line(&kind, &host, port, &user, &pass)
    } else {
        // A saved proxy, by id.
        let data = data_of(send_json(c.get(format!("{base}/proxies/list")), WHAT, HINT).await?)?;
        let saved: HashMap<String, Value> = data
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        match saved.get(raw.trim()) {
            Some(p) => proxy_line(
                &text_of(p.get("type")),
                &text_of(p.get("host")),
                num_of(p.get("port")),
                &text_of(p.get("login")),
                &text_of(p.get("password")),
            ),
            None => (None, Some(format!("the saved proxy {raw:?} is not in Undetectable's proxy list"))),
        }
    };

    let data = data_of(send_json(c.get(format!("{base}/profile/cookies/{}", summary.id)), WHAT, HINT).await?)?;
    let cookies: Vec<Value> = data
        .get("cookies")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|c| c.is_object()).cloned().collect())
        .unwrap_or_default();
    let cookie_note = cookies.is_empty().then(|| "Undetectable has no cookies stored for this profile".to_string());
    Ok(Detail { proxy_line, proxy_note, start_url: None, cookies, cookie_note })
}

#[cfg(test)]
mod tests {
    use super::super::stand_in::serve;
    use super::*;

    #[test]
    fn the_proxy_string_is_read_with_a_colon_in_the_password() {
        assert_eq!(
            parse("socks5://127.0.0.1:5555:login:pa:ss"),
            Some(("socks5".into(), "127.0.0.1".into(), 5555, "login".into(), "pa:ss".into()))
        );
        assert_eq!(parse("http://h:80"), Some(("http".into(), "h".into(), 80, String::new(), String::new())));
        assert_eq!(parse("21247902"), None);
    }

    #[tokio::test]
    async fn the_keyed_list_is_read_with_each_profiles_detail() {
        let (base, _) = serve(vec![
            ("GET", "/list", 200, json!({"code": 0, "status": "success", "data": {
                "52199655686a7a3d": {"name": "Profile1", "status": "Available", "tags": ["t1"]}}})),
            ("GET", "/profile/getinfo/52199655686a7a3d", 200, json!({"code": 0, "data": {
                "name": "Profile1", "notes": "Text", "os": "Windows 10", "tags": ["t1", "t2"],
                "proxy": "socks5://127.0.0.1:5555:login:pass"}})),
        ])
        .await;
        let got = list(&base).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].os, "win");
        assert_eq!(got[0].notes, "Text");
        assert_eq!(got[0].tags, vec!["t1", "t2"]);
        assert_eq!(got[0].proxy.as_deref(), Some("127.0.0.1:5555"));
    }

    #[tokio::test]
    async fn a_saved_proxy_id_is_resolved_and_cookies_read() {
        let (base, _) = serve(vec![
            ("GET", "/proxies/list", 200, json!({"code": 0, "data": {"21247902": {
                "host": "192.0.2.5", "login": "pl", "password": "wifi;us;;;", "port": 9168, "type": "socks5"}}})),
            ("GET", "/profile/cookies/p1", 200, json!({"code": 0, "data": {"cookies": [
                {"domain": "google.com", "expirationDate": 1893456000, "httpOnly": true, "name": "PHPSESSID",
                 "path": "/", "samesite": "Unspecefied", "secure": true, "session": false, "value": "f6"}, "..."]}})),
        ])
        .await;
        let s = Summary { id: "p1".into(), name: "x".into(), notes: String::new(), tags: vec![], os: String::new(),
                          proxy: None, extra: json!({"proxy": "21247902"}) };
        let d = profile(&base, &s).await.unwrap();
        assert_eq!(d.proxy_line.as_deref(), Some("socks5://pl:wifi;us;;;@192.0.2.5:9168"));
        assert_eq!(d.cookies.len(), 1, "the placeholder string in the array is skipped");
    }

    #[tokio::test]
    async fn an_error_envelope_says_undetectables_reason() {
        let (base, _) = serve(vec![("GET", "/list", 200, json!({"code": 1, "status": "error", "data": {"error": "not logged in"}}))]).await;
        let e = format!("{:#}", list(&base).await.unwrap_err());
        assert!(e.contains("not logged in"), "{e}");
    }
}
