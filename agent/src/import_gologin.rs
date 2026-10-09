// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Moving profiles out of GoLogin.
//!
//! Asked for by somebody who would switch and does not want to carry every
//! profile and its cookies across by hand. GoLogin keeps both on its own
//! servers and publishes an API for them (`docs-download.gologin.com`,
//! OpenAPI, bearer token from "API & MCP" in their app), so the move reads
//! from there rather than from anything on disk:
//!
//! - `GET /browser/v2?page=N` — the profile list, 30 to a page;
//! - `GET /browser/{id}` — one profile, the only place the proxy password is;
//! - `GET /browser/{id}/cookies` — the jar, already decrypted, in the format
//!   browser extensions emit. `cookies::prepare` reads that format, so the
//!   cookies go into `profile.cookies.import` unchanged.
//!
//! ## What does not travel, on purpose
//!
//! **The fingerprint.** GoLogin's profile is a set of independent values
//! (a user agent here, a WebGL renderer there); ours is a machine from the
//! catalogue, and catalogue.rs says why the two do not mix. The shell gives
//! the new profile a persona of the same OS, which is the part a site
//! remembers. The account sees a new device on the same proxy, which is what
//! moving to any other anti-detect browser looks like.
//!
//! **Local storage and IndexedDB.** GoLogin keeps them inside a zipped Orbita
//! profile on its storage, not behind the API. Cookies are what keeps an
//! account signed in; the rest is said in the result rather than implied.
//!
//! ## The token
//!
//! Taken per call and dropped with it: never written to the store, never in a
//! log line, never in an error message. It opens the whole GoLogin account,
//! proxies with passwords included.

use anyhow::{bail, Context, Result};
use serde_json::Value;

pub const API: &str = "https://api.gologin.com";

/// `FURY_GOLOGIN_API` points the import at a stand-in for end-to-end tests.
/// An environment variable of the agent's own process, so only whoever starts
/// the agent can set it; a page or a peer cannot.
fn base() -> String {
    std::env::var("FURY_GOLOGIN_API").unwrap_or_else(|_| API.to_string())
}

/// One GoLogin profile, as much as the list shows.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub notes: String,
    pub tags: Vec<String>,
    /// GoLogin's own word: "win", "mac", "lin", "android".
    pub os: String,
    /// `host:port` for showing, or None when the profile has no proxy.
    pub proxy: Option<String>,
}

/// What `profile` returns: the parts the list does not carry.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Detail {
    /// A proxy line the paste parser reads, credentials included.
    pub proxy_line: Option<String>,
    /// Why the proxy is not in `proxy_line`, when the profile has one.
    pub proxy_note: Option<String>,
    pub start_url: Option<String>,
    pub cookies: Vec<Value>,
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?)
}

async fn get(client: &reqwest::Client, base: &str, path: &str, token: &str) -> Result<Value> {
    let response = client
        .get(format!("{base}{path}"))
        .bearer_auth(token)
        .header("User-Agent", concat!("fury-agent/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .context("GoLogin did not answer")?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        bail!(
            "GoLogin refused the token ({status}). Copy it again from GoLogin: \
             API & MCP, the API tab"
        );
    }
    if !status.is_success() {
        bail!("GoLogin answered {status} for {path}");
    }
    response.json().await.with_context(|| format!("GoLogin's answer for {path} is not JSON"))
}

/// Every profile in the account, page by page.
pub async fn list(token: &str) -> Result<Vec<Summary>> {
    list_from(&base(), token).await
}

pub async fn list_from(base: &str, token: &str) -> Result<Vec<Summary>> {
    let client = client()?;
    let mut out: Vec<Summary> = Vec::new();
    // A ceiling, not an expectation: 30 to a page, so 400 pages is 12 000
    // profiles. An API that kept answering with the same page would otherwise
    // be a loop with no end.
    for page in 1..=400u32 {
        let body = get(&client, base, &format!("/browser/v2?page={page}"), token).await?;
        let (profiles, total) = parse_list(&body)?;
        if profiles.is_empty() {
            break;
        }
        let before = out.len();
        for p in profiles {
            if !out.iter().any(|o| o.id == p.id) {
                out.push(p);
            }
        }
        if out.len() == before || total.is_some_and(|t| out.len() >= t) {
            break;
        }
    }
    Ok(out)
}

/// One profile's proxy, start page and cookies.
pub async fn profile(token: &str, id: &str) -> Result<Detail> {
    profile_from(&base(), token, id).await
}

pub async fn profile_from(base: &str, token: &str, id: &str) -> Result<Detail> {
    // The id goes into a path; GoLogin's are 24 hex characters, and anything
    // else is not one of theirs.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("{id:?} is not a GoLogin profile id");
    }
    let client = client()?;
    let body = get(&client, base, &format!("/browser/{id}"), token).await?;
    let (proxy_line, proxy_note) = proxy_of(&body);
    let start_url = body
        .get("startUrl")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let jar = get(&client, base, &format!("/browser/{id}/cookies"), token).await?;
    let cookies = match jar {
        Value::Array(items) => items,
        // Some deployments wrap it; take the array wherever it is.
        Value::Object(ref o) => o
            .get("cookies")
            .or_else(|| o.get("data"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    Ok(Detail { proxy_line, proxy_note, start_url, cookies })
}

/// The profiles on one page of `/browser/v2`, and the total if it says one.
pub fn parse_list(body: &Value) -> Result<(Vec<Summary>, Option<usize>)> {
    let profiles = body
        .get("profiles")
        .and_then(Value::as_array)
        .context("GoLogin's profile list has no \"profiles\" array")?;
    let total = body.get("allProfilesCount").and_then(Value::as_u64).map(|n| n as usize);
    let mut out = Vec::new();
    for p in profiles {
        let Some(id) = p.get("id").and_then(Value::as_str) else { continue };
        let text = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let proxy = p.get("proxy").filter(|x| x.is_object()).unwrap_or(p);
        let host = proxy.get("host").and_then(Value::as_str).unwrap_or("").trim();
        let port = proxy.get("port").and_then(Value::as_u64).unwrap_or(0);
        let mode = proxy.get("mode").and_then(Value::as_str).unwrap_or("");
        out.push(Summary {
            id: id.to_string(),
            name: text("name"),
            notes: text("notes"),
            tags: tags_of(p.get("tags")),
            os: os_of(p.get("os")),
            proxy: (!host.is_empty() && port > 0 && mode != "none").then(|| format!("{host}:{port}")),
        });
    }
    Ok((out, total))
}

/// Tags arrive as strings or as objects with a title, depending on the
/// endpoint's version; both are read.
fn tags_of(v: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = v else { return Vec::new() };
    items
        .iter()
        .filter_map(|t| match t {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => o
                .get("title")
                .or_else(|| o.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `os` is a string on the profile and, per the spec, an object in the list.
fn os_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => o
            .get("os")
            .or_else(|| o.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    }
}

/// The proxy as a line the paste parser reads, or why there is none.
///
/// Only the modes that are an address the operator owns become a line.
/// GoLogin's own pool ("gologin", "tor", "geolocation") is a service of their
/// account, and its credentials, where the API shows them, are rented from
/// GoLogin and stop working when the subscription does — carrying them would
/// be a proxy that dies on its own a month later.
pub fn proxy_of(profile: &Value) -> (Option<String>, Option<String>) {
    let Some(p) = profile.get("proxy").filter(|p| p.is_object()) else {
        return (None, None);
    };
    let mode = p.get("mode").and_then(Value::as_str).unwrap_or("").to_lowercase();
    let host = p.get("host").and_then(Value::as_str).unwrap_or("").trim();
    let port = p.get("port").and_then(Value::as_u64).unwrap_or(0);
    let user = p.get("username").and_then(Value::as_str).unwrap_or("");
    let pass = p.get("password").and_then(Value::as_str).unwrap_or("");
    match mode.as_str() {
        "" | "none" => (None, None),
        // socks4 is left to the parser, which refuses it with the reason.
        "http" | "https" | "socks5" | "socks4" if !host.is_empty() && port > 0 => {
            // Unescaped: the paste parser splits on the LAST `@` and the
            // FIRST `:`, so a password with either arrives whole, and it does
            // not decode `%XX` — escaping here would store the escapes. The
            // one shape it cannot carry is a colon in the username.
            if user.contains(':') {
                return (None, Some("the proxy username contains a colon".into()));
            }
            let auth = if user.is_empty() {
                String::new()
            } else {
                format!("{user}:{pass}@")
            };
            (Some(format!("{mode}://{auth}{host}:{port}")), None)
        }
        "http" | "https" | "socks5" | "socks4" => {
            (None, Some("the proxy has no address in GoLogin".into()))
        }
        other => (
            None,
            Some(format!(
                "a GoLogin {other} proxy belongs to the GoLogin subscription and is not carried"
            )),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_list_page_reads_names_tags_os_and_proxy() {
        let body = json!({
            "allProfilesCount": 2,
            "profiles": [
                {"id": "65a1b2c3d4e5f6a7b8c9d0e1", "name": " Shop 1 ", "notes": "main",
                 "os": {"os": "win"}, "tags": [{"title": "de", "color": "red"}, "warm"],
                 "proxy": {"mode": "http", "host": "1.2.3.4", "port": 8080}},
                {"id": "65a1b2c3d4e5f6a7b8c9d0e2", "name": "Mac", "os": "mac",
                 "proxy": {"mode": "none"}},
                {"name": "no id, skipped"}
            ]
        });
        let (p, total) = parse_list(&body).unwrap();
        assert_eq!(total, Some(2));
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name, "Shop 1");
        assert_eq!(p[0].os, "win");
        assert_eq!(p[0].tags, vec!["de", "warm"]);
        assert_eq!(p[0].proxy.as_deref(), Some("1.2.3.4:8080"));
        assert_eq!(p[1].os, "mac");
        assert_eq!(p[1].proxy, None);
    }

    #[test]
    fn a_list_without_profiles_is_an_error_not_an_empty_account() {
        assert!(parse_list(&json!({"message": "nope"})).is_err());
    }

    #[test]
    fn an_own_proxy_becomes_a_line_the_paste_parser_reads_back() {
        let (line, note) = proxy_of(&json!({"proxy": {
            "mode": "socks5", "host": "5.6.7.8", "port": 1080,
            "username": "u@x", "password": "p:a%ss"
        }}));
        assert_eq!(line.as_deref(), Some("socks5://u@x:p:a%ss@5.6.7.8:1080"));
        assert_eq!(note, None);
        // and the paste parser reads it back
        let parsed = fury_shared::proxy_list::parse_line(line.as_deref().unwrap()).unwrap();
        assert_eq!(parsed.host, "5.6.7.8");
        assert_eq!(parsed.port, 1080);
        assert_eq!(parsed.username.as_deref(), Some("u@x"));
        assert_eq!(parsed.password.as_deref(), Some("p:a%ss"));
    }

    #[test]
    fn gologins_own_pool_is_named_not_carried() {
        let (line, note) = proxy_of(&json!({"proxy": {"mode": "gologin", "host": "x", "port": 1}}));
        assert_eq!(line, None);
        assert!(note.unwrap().contains("gologin"));
        assert_eq!(proxy_of(&json!({"proxy": {"mode": "none"}})), (None, None));
        assert_eq!(proxy_of(&json!({})), (None, None));
    }

    /// One-shot HTTP server on loopback answering by path, recording requests.
    async fn mock(
        answers: Vec<(&'static str, u16, Value)>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                log.lock().unwrap().push(req);
                let (code, body) = answers
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, c, b)| (*c, b.to_string()))
                    .unwrap_or((404, "{}".into()));
                let reply = format!(
                    "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes()).await;
            }
        });
        (base, seen)
    }

    #[tokio::test]
    async fn list_follows_pages_until_the_count_and_sends_the_token() {
        let page = |ids: &[&str]| {
            json!({"allProfilesCount": 3, "profiles": ids.iter().map(|i| json!({"id": i, "name": i, "os": "win"})).collect::<Vec<_>>()})
        };
        let (base, seen) = mock(vec![
            ("/browser/v2?page=1", 200, page(&["a1", "a2"])),
            ("/browser/v2?page=2", 200, page(&["a3"])),
        ])
        .await;
        let got = list_from(&base, "tok123").await.unwrap();
        assert_eq!(got.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["a1", "a2", "a3"]);
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 2, "stopped at the count, no third page");
        assert!(reqs[0].to_lowercase().contains("authorization: bearer tok123"));
    }

    #[tokio::test]
    async fn a_refused_token_says_where_to_get_one_and_does_not_echo_it() {
        let (base, _) = mock(vec![("/browser/v2?page=1", 401, json!({"message": "bad"}))]).await;
        let e = format!("{:#}", list_from(&base, "secret-token").await.unwrap_err());
        assert!(e.contains("API & MCP"), "{e}");
        assert!(!e.contains("secret-token"), "{e}");
    }

    #[tokio::test]
    async fn a_profile_brings_its_proxy_start_page_and_cookies() {
        let (base, _) = mock(vec![
            ("/browser/abc123", 200, json!({"id": "abc123", "startUrl": " https://mail.example ",
                "proxy": {"mode": "http", "host": "1.2.3.4", "port": 3128, "username": "u", "password": "p"}})),
            ("/browser/abc123/cookies", 200, json!([
                {"domain": ".example.com", "name": "sid", "value": "v", "path": "/",
                 "secure": true, "httpOnly": true, "sameSite": "no_restriction",
                 "hostOnly": false, "session": false, "expirationDate": 1893456000.5}
            ])),
        ])
        .await;
        let d = profile_from(&base, "t", "abc123").await.unwrap();
        assert_eq!(d.proxy_line.as_deref(), Some("http://u:p@1.2.3.4:3128"));
        assert_eq!(d.start_url.as_deref(), Some("https://mail.example"));
        assert_eq!(d.cookies.len(), 1);
        // the format cookies.rs already reads
        let c = crate::cookies::prepare(&d.cookies).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0]["sameSite"], "None");
        assert_eq!(c[0]["expires"], 1893456000.5);
    }

    #[tokio::test]
    async fn an_id_that_is_not_gologins_never_reaches_a_url() {
        assert!(profile_from("http://127.0.0.1:9", "t", "../users").await.is_err());
    }
}
