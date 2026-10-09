// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! AdsPower, through its Local API (localapi-doc-en.adspower.com, and the
//! contracts in AdsPower's own local-api-mcp-typescript). AdsPower has to be
//! running on this machine, on a paid plan; the key is needed only when its
//! "security verification" is on.
//!
//! - Every answer is HTTP 200 with `{code, msg, data}`; `code` other than 0
//!   is the failure, and `msg` says why.
//! - The list defaults to ONE profile a page, so `limit` is always sent.
//! - The proxy, password included, is in the list as `user_proxy_config`,
//!   unless the profile points at a saved proxy (`fbcc_proxy_acc_id`), which
//!   the proxy list resolves.
//! - Cookies are a JSON string inside the JSON, read one profile a second
//!   (the documented limit for that route). `session: true` appears on every
//!   cookie of the documented example alongside a real expiry, so the expiry
//!   is what counts: `normalise_cookies` reads `expires` when `session` is
//!   false, and here `session` is dropped before that.
//! - Tags, the OS and start pages are not in the documented list.

use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::{client, num_of, plain_text, proxy_line, send_json, text_of, Detail, Summary};

pub const BASE: &str = "http://127.0.0.1:50325";
const WHAT: &str = "AdsPower";
const HINT: &str = "Copy the key from AdsPower's Local API settings, or leave the field empty if security verification is off";

fn with_key(r: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    if key.trim().is_empty() { r } else { r.bearer_auth(key.trim()) }
}

/// The envelope: `data` when `code` is 0, AdsPower's message otherwise.
fn data_of(body: Value) -> Result<Value> {
    if body.get("code").and_then(Value::as_i64) != Some(0) {
        let msg = text_of(body.get("msg"));
        bail!("{WHAT} refused: {}", if msg.is_empty() { "no reason given".into() } else { msg });
    }
    Ok(body.get("data").cloned().unwrap_or(Value::Null))
}

pub async fn list(base: &str, key: &str) -> Result<Vec<Summary>> {
    let c = client()?;
    let mut out = Vec::new();
    const LIMIT: u64 = 100;
    for page in 1..=1000u64 {
        let body = send_json(
            with_key(c.post(format!("{base}/api/v2/browser-profile/list")), key)
                .json(&json!({ "page": page, "limit": LIMIT })),
            WHAT,
            HINT,
        )
        .await?;
        let data = data_of(body)?;
        let items = data.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
        out.extend(items.iter().filter_map(summary_of));
        let total_pages = num_of(data.get("total_pages"));
        if (items.len() as u64) < LIMIT || (total_pages > 0 && page >= total_pages) {
            break;
        }
    }
    Ok(out)
}

pub(crate) fn summary_of(p: &Value) -> Option<Summary> {
    let id = text_of(p.get("profile_id"));
    if id.is_empty() {
        return None;
    }
    let cfg = p.get("user_proxy_config").cloned().unwrap_or(Value::Null);
    let host = text_of(cfg.get("proxy_host"));
    let port = num_of(cfg.get("proxy_port"));
    let saved = text_of(p.get("fbcc_proxy_acc_id"));
    let none = text_of(cfg.get("proxy_soft")) == "no_proxy" || text_of(cfg.get("proxy_type")) == "no_proxy";
    let name = text_of(p.get("name"));
    Some(Summary {
        name: if name.is_empty() { format!("AdsPower {}", text_of(p.get("profile_no"))) } else { name },
        notes: plain_text(&text_of(p.get("remark"))),
        tags: Vec::new(),
        os: String::new(),
        proxy: if !saved.is_empty() {
            Some("saved proxy".into())
        } else if !none && !host.is_empty() {
            Some(format!("{host}:{port}"))
        } else {
            None
        },
        extra: json!({ "user_proxy_config": cfg, "fbcc_proxy_acc_id": saved }),
        id,
    })
}

pub async fn profile(base: &str, key: &str, summary: &Summary) -> Result<Detail> {
    let c = client()?;

    let saved = text_of(summary.extra.get("fbcc_proxy_acc_id"));
    let (kind, host, port, user, pass) = if !saved.is_empty() {
        let body = send_json(
            with_key(c.post(format!("{base}/api/v2/proxy-list/list")), key)
                .json(&json!({ "proxy_id": [saved], "limit": 1, "page": 1 })),
            WHAT,
            HINT,
        )
        .await?;
        let data = data_of(body)?;
        let p = data.get("list").and_then(Value::as_array).and_then(|l| l.first()).cloned().unwrap_or(Value::Null);
        (text_of(p.get("type")), text_of(p.get("host")), num_of(p.get("port")), text_of(p.get("user")), text_of(p.get("password")))
    } else {
        let p = summary.extra.get("user_proxy_config").cloned().unwrap_or(Value::Null);
        (
            text_of(p.get("proxy_type")),
            text_of(p.get("proxy_host")),
            num_of(p.get("proxy_port")),
            text_of(p.get("proxy_user")),
            text_of(p.get("proxy_password")),
        )
    };
    let (proxy_line, proxy_note) = if host.is_empty() || kind == "no_proxy" {
        (None, None)
    } else {
        proxy_line(&kind, &host, port, &user, &pass)
    };

    // One a second: the cookie route's documented limit.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let body = send_json(
        with_key(c.get(format!("{base}/api/v2/browser-profile/cookies")), key)
            .query(&[("profile_id", summary.id.as_str())]),
        WHAT,
        HINT,
    )
    .await?;
    let data = data_of(body)?;
    let mut cookies = match data.get("cookies") {
        Some(Value::String(s)) if !s.trim().is_empty() => serde_json::from_str::<Vec<Value>>(s).unwrap_or_default(),
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    for c in cookies.iter_mut() {
        if let Some(o) = c.as_object_mut() {
            o.remove("session");
        }
    }
    let cookie_note = cookies.is_empty().then(|| "AdsPower has no cookies stored for this profile".to_string());
    Ok(Detail { proxy_line, proxy_note, start_url: None, cookies, cookie_note })
}

#[cfg(test)]
mod tests {
    use super::super::stand_in::serve;
    use super::*;

    #[tokio::test]
    async fn the_list_reads_inline_and_saved_proxies_and_stops_on_a_short_page() {
        let (base, seen) = serve(vec![
            ("POST", "/api/v2/browser-profile/list", 200, json!({"code": 0, "msg": "Success", "data": {"list": [
                {"profile_id": "h1yynkm", "profile_no": "123", "name": "Shop 1", "remark": "main",
                 "fbcc_proxy_acc_id": "", "user_proxy_config": {"proxy_soft": "other", "proxy_type": "socks5",
                 "proxy_host": "pr.example.io", "proxy_port": "123", "proxy_user": "abc", "proxy_password": "xyz"}},
                {"profile_id": "k2", "profile_no": "124", "name": "", "fbcc_proxy_acc_id": "77",
                 "user_proxy_config": {"proxy_soft": "no_proxy"}}
            ], "page": 1, "limit": 100}})),
        ])
        .await;
        let got = list(&base, "").await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].proxy.as_deref(), Some("pr.example.io:123"));
        assert_eq!(got[1].name, "AdsPower 124");
        let req = &seen.all()[0];
        assert!(req.contains(r#""limit":100"#), "{req}");
        assert!(!req.to_lowercase().contains("authorization"), "no key, no header");
    }

    #[tokio::test]
    async fn a_refusal_in_the_envelope_is_an_error_with_adspowers_words() {
        let (base, _) = serve(vec![("POST", "/api/v2/browser-profile/list", 200,
            json!({"code": -1, "msg": "API key is invalid", "data": {}}))]).await;
        let e = format!("{:#}", list(&base, "k").await.unwrap_err());
        assert!(e.contains("API key is invalid"), "{e}");
    }

    #[tokio::test]
    async fn a_profile_resolves_a_saved_proxy_and_parses_the_cookie_string() {
        let cookies = r#"[{"name":"acw","value":"1","domain":"www.adspower.net","path":"/","httpOnly":true,"secure":false,"session":true,"expires":1893456000,"sameSite":"unspecified"}]"#;
        let (base, seen) = serve(vec![
            ("POST", "/api/v2/proxy-list/list", 200, json!({"code": 0, "data": {"list": [
                {"proxy_id": "77", "type": "https", "host": "192.0.2.1", "port": "8001", "user": "u", "password": "p"}]}})),
            ("GET", "/api/v2/browser-profile/cookies?profile_id=k2", 200, json!({"code": 0, "data": {"cookies": cookies}})),
        ])
        .await;
        let s = Summary { id: "k2".into(), name: "x".into(), notes: String::new(), tags: vec![], os: String::new(),
                          proxy: None, extra: json!({"fbcc_proxy_acc_id": "77"}) };
        let d = profile(&base, "key1", &s).await.unwrap();
        assert_eq!(d.proxy_line.as_deref(), Some("https://u:p@192.0.2.1:8001"));
        assert_eq!(d.cookies.len(), 1);
        let n = super::super::normalise_cookies(&d.cookies);
        assert_eq!(n[0]["expires"], 1893456000.0, "session:true beside a real expiry is not a session cookie");
        assert!(seen.all()[0].to_lowercase().contains("authorization: bearer key1"));
    }
}
