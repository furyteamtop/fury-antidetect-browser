// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Moving profiles out of other anti-detect browsers, through their own APIs.
//!
//! Each vendor module reads two things: the profile list, and for one profile
//! its proxy (with the password), start page and cookie jar. Nothing is
//! written anywhere by this module. The shell creates every profile the
//! ordinary way, `proxies.importMany` then `profiles.upsert`, and hands the
//! cookies to `profile.cookies.import`, exactly as the CSV import and the
//! cookie dialog do.
//!
//! What every vendor has in common, and is decided once here:
//!
//! - **The fingerprint does not travel.** Theirs is a set of independent
//!   values; ours is a machine from the catalogue (catalogue.rs says why the
//!   two do not mix). The shell gives the new profile a device of the same OS.
//! - **Their own proxy pools do not travel.** A proxy rented through the
//!   vendor's subscription stops working with it; such a profile comes over
//!   without one, and says why in `proxy_note`.
//! - **The token is kept nowhere.** It is a parameter of each call, never in
//!   the store, a log line or an error message.
//! - **Cookies are normalised here**, into the shape `cookies::prepare`
//!   reads: every vendor spells sameSite, expiry and host-only differently,
//!   and a jar that half-imports looks like a jar that imported.

use anyhow::{bail, Context, Result};
use serde_json::Value;

pub mod adspower;
pub mod dolphin;
pub mod gologin;
pub mod kameleo;
pub mod undetectable;
pub mod vision;

/// One profile in the source, as much as its list shows.
///
/// `extra` is whatever that vendor needs back to fetch the profile's detail
/// (a folder id, a reference to a saved proxy). The shell does not read it;
/// it passes the whole summary back to `import.vendor.profile`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Summary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// "win" | "mac" | "lin" | "android", or "" when the source does not say.
    #[serde(default)]
    pub os: String,
    /// `host:port` for showing, or None when the profile has no proxy.
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default)]
    pub extra: Value,
}

/// What `profile` returns: the parts the list does not carry.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Detail {
    /// A proxy line the paste parser reads, credentials included.
    pub proxy_line: Option<String>,
    /// Why the proxy is not in `proxy_line`, when the profile has one.
    pub proxy_note: Option<String>,
    pub start_url: Option<String>,
    /// Already normalised (`normalise_cookies`).
    pub cookies: Vec<Value>,
    /// Why there are no cookies, or fewer than there might be, when the
    /// source says so: cloud sync off, a running profile, a password.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie_note: Option<String>,
}

/// The sources this agent can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    GoLogin,
    Dolphin,
    AdsPower,
    Undetectable,
    Kameleo,
    Vision,
}

impl Vendor {
    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name {
            "gologin" => Vendor::GoLogin,
            "dolphin" => Vendor::Dolphin,
            "adspower" => Vendor::AdsPower,
            "undetectable" => Vendor::Undetectable,
            "kameleo" => Vendor::Kameleo,
            "vision" => Vendor::Vision,
            other => bail!("{other:?} is not a source this version can import from"),
        })
    }
}

/// How to reach a vendor: its token, and where its API answers when that is
/// not the default (a local API on a port the person changed).
#[derive(Debug, Clone, Default)]
pub struct Access {
    pub token: String,
    pub base: Option<String>,
}

pub async fn list(vendor: Vendor, access: &Access) -> Result<Vec<Summary>> {
    let t = &access.token;
    match vendor {
        Vendor::GoLogin => gologin::list_from(&access.base_or(&gologin::base()), t).await,
        Vendor::Dolphin => dolphin::list(&dolphin::Hosts::for_base(access.base.as_deref()), t).await,
        Vendor::AdsPower => adspower::list(&access.base_or(adspower::BASE), t).await,
        Vendor::Undetectable => undetectable::list(&access.base_or(undetectable::BASE)).await,
        Vendor::Kameleo => kameleo::list(&access.base_or(kameleo::BASE)).await,
        Vendor::Vision => vision::list(&access.base_or(vision::BASE), t).await,
    }
}

pub async fn profile(vendor: Vendor, access: &Access, summary: &Summary) -> Result<Detail> {
    let t = &access.token;
    let mut detail = match vendor {
        Vendor::GoLogin => {
            gologin::profile_from(&access.base_or(&gologin::base()), t, &summary.id).await?
        }
        Vendor::Dolphin => {
            dolphin::profile(&dolphin::Hosts::for_base(access.base.as_deref()), t, summary).await?
        }
        Vendor::AdsPower => adspower::profile(&access.base_or(adspower::BASE), t, summary).await?,
        Vendor::Undetectable => undetectable::profile(&access.base_or(undetectable::BASE), summary).await?,
        Vendor::Kameleo => kameleo::profile(&access.base_or(kameleo::BASE), summary).await?,
        Vendor::Vision => vision::profile(&access.base_or(vision::BASE), t, summary).await?,
    };
    detail.cookies = normalise_cookies(&detail.cookies);
    Ok(detail)
}

impl Access {
    fn base_or(&self, default: &str) -> String {
        self.base
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .unwrap_or(default)
            .trim_end_matches('/')
            .to_string()
    }
}

// ---- shared HTTP ----------------------------------------------------------

pub(crate) fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?)
}

/// Sends a request and reads JSON back, turning the failures every vendor
/// shares into sentences: a refused token says where to get a new one, an
/// unreachable local API says the app must be running. `what` names the
/// vendor in those sentences; the token never appears in any of them.
pub(crate) async fn send_json(
    request: reqwest::RequestBuilder,
    what: &str,
    token_hint: &str,
) -> Result<Value> {
    let (status, body) = send(request, what, token_hint).await?;
    if !status.is_success() {
        bail!("{what} answered {status}{}", error_text(&body).map(|t| format!(": {t}")).unwrap_or_default());
    }
    Ok(body)
}

/// The vendor's own words for an error, wherever its envelope keeps them.
pub(crate) fn error_text(body: &Value) -> Option<String> {
    [
        body.pointer("/error/text"),
        body.pointer("/msg"),
        body.pointer("/message"),
        body.pointer("/title"),
        body.pointer("/data/error"),
        body.pointer("/error"),
    ]
    .into_iter()
    .flatten()
    .find_map(|v| v.as_str().map(str::to_string))
    .filter(|t| !t.trim().is_empty())
}

/// Like `send_json`, but a status that is neither an auth nor a rate refusal
/// comes back to the caller with its body, for routes whose 404 or 409 means
/// something the vendor module has to act on.
pub(crate) async fn send(
    request: reqwest::RequestBuilder,
    what: &str,
    token_hint: &str,
) -> Result<(reqwest::StatusCode, Value)> {
    let response = request
        .header("User-Agent", concat!("fury-agent/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() {
                anyhow::anyhow!(
                    "{what} did not answer. If its API is local, the {what} app has to be \
                     running and signed in"
                )
            } else {
                anyhow::anyhow!("{what} did not answer: {e}")
            }
        })?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        bail!("{what} refused the token ({status}). {token_hint}");
    }
    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        bail!("{what} answered 402: its API is not part of this account's plan");
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        bail!("{what} asked to slow down (429). Wait a minute and run the import again");
    }
    let text = response.text().await.with_context(|| format!("{what}'s answer could not be read"))?;
    let body = if text.trim().is_empty() {
        Value::Null
    } else {
        match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) if !status.is_success() => Value::Null,
            Err(e) => bail!("{what}'s answer is not JSON: {e}"),
        }
    };
    Ok((status, body))
}

/// The text of an HTML-ish note: tags out, entities for the five that
/// matter decoded, runs of whitespace collapsed. Notes in some sources are
/// rich text, and a profile's notes in Fury are plain.
pub(crate) fn plain_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A JSON value read as text whether the source sent a string or a number:
/// several APIs send ids and ports as either.
pub(crate) fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// And as a number, from either.
pub(crate) fn num_of(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

// ---- shared shapes ----------------------------------------------------------

/// A proxy line the paste parser reads, or why there is none.
///
/// Unescaped on purpose: the paste parser splits on the LAST `@` and the FIRST
/// `:`, so a password with either arrives whole, and it does not decode `%XX`.
/// The one shape it cannot carry is a colon in the username.
pub(crate) fn proxy_line(
    kind: &str,
    host: &str,
    port: u64,
    user: &str,
    pass: &str,
) -> (Option<String>, Option<String>) {
    let kind = kind.trim().to_ascii_lowercase();
    let kind = match kind.as_str() {
        "socks" | "socks5h" => "socks5".to_string(),
        other => other.to_string(),
    };
    let host = host.trim();
    if host.is_empty() || port == 0 {
        return (None, Some("the proxy has no address in the source".into()));
    }
    if !matches!(kind.as_str(), "http" | "https" | "socks5" | "socks4") {
        return (None, Some(format!("a {kind} proxy is not one Fury can use")));
    }
    if user.contains(':') {
        return (None, Some("the proxy username contains a colon".into()));
    }
    let auth = if user.is_empty() { String::new() } else { format!("{user}:{pass}@") };
    (Some(format!("{kind}://{auth}{host}:{port}")), None)
}

/// The source's word for an OS, as the shell reads it.
pub(crate) fn os_word(raw: &str) -> String {
    let l = raw.trim().to_ascii_lowercase();
    if l.starts_with("win") {
        "win".into()
    } else if l.starts_with("mac") || l == "darwin" || l == "osx" || l.starts_with("os x") {
        "mac".into()
    } else if l.contains("android") {
        "android".into()
    } else if l.starts_with("lin") || l == "ubuntu" {
        "lin".into()
    } else {
        String::new()
    }
}

/// Every vendor's cookie into the shape `cookies::prepare` reads.
///
/// What differs between them, all of it measured off their specifications:
/// sameSite spelled `sameSite`, `samesite` or `same_site`, with
/// `no_restriction`, `unspecified` or "Unspecefied" (sic) as values; expiry
/// as `expirationDate` or `expires`, in seconds or milliseconds, sometimes a
/// string; host-only cookies flagged `hostOnly`. A host-only cookie set with
/// a `domain` becomes a domain cookie, sent to every subdomain, so it goes in
/// through `url` instead, which is how CDP makes a host-only one.
pub fn normalise_cookies(cookies: &[Value]) -> Vec<Value> {
    cookies.iter().filter_map(normalise_cookie).collect()
}

fn normalise_cookie(raw: &Value) -> Option<Value> {
    let o = raw.as_object()?;
    let text = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
    let flag = |keys: &[&str]| {
        keys.iter().find_map(|k| match o.get(*k) {
            Some(Value::Bool(b)) => Some(*b),
            Some(Value::Number(n)) => Some(n.as_f64() != Some(0.0)),
            Some(Value::String(s)) => Some(s == "true" || s == "1"),
            _ => None,
        })
    };

    let name = text("name")?;
    let value = text("value").unwrap_or_default();
    let domain = text("domain").or_else(|| text("host")).unwrap_or_default();
    if name.is_empty() || domain.is_empty() {
        return None;
    }
    let path = text("path").filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into());
    let secure = flag(&["secure"]).unwrap_or(false);
    let http_only = flag(&["httpOnly", "httponly", "http_only"]).unwrap_or(false);
    let session = flag(&["session"]).unwrap_or(false);
    let host_only = flag(&["hostOnly", "host_only"]).unwrap_or(false);

    let mut out = serde_json::Map::new();
    out.insert("name".into(), name.into());
    out.insert("value".into(), value.into());
    out.insert("path".into(), path.clone().into());
    out.insert("secure".into(), secure.into());
    out.insert("httpOnly".into(), http_only.into());

    if host_only && !domain.starts_with('.') {
        let scheme = if secure { "https" } else { "http" };
        out.insert("url".into(), format!("{scheme}://{domain}{path}").into());
    } else {
        out.insert("domain".into(), domain.into());
    }

    let same_site = text("sameSite").or_else(|| text("samesite")).or_else(|| text("same_site"));
    if let Some(s) = same_site {
        // Vision writes "norestriction"; cookies::prepare knows the
        // extension spelling, and drops what it does not know, which would
        // turn a SameSite=None cookie into the browser's Lax default.
        let s = if s.eq_ignore_ascii_case("norestriction") { "no_restriction".to_string() } else { s };
        out.insert("sameSite".into(), s.into());
    }

    if !session {
        let expiry = ["expirationDate", "expires", "expiry", "expiration_date"]
            .iter()
            .find_map(|k| match o.get(*k) {
                Some(Value::Number(n)) => n.as_f64(),
                Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
                _ => None,
            });
        if let Some(mut e) = expiry {
            // Milliseconds: no cookie expires after the year 5138.
            if e > 1e11 {
                e /= 1000.0;
            }
            if e > 0.0 {
                out.insert("expires".into(), serde_json::json!(e));
            }
        }
    }
    Some(Value::Object(out))
}

/// A stand-in vendor API on loopback for the vendor modules' tests: answers by
/// method and path (query included), and records every request it saw, body
/// included, so a test can check what was sent as well as what was read.
#[cfg(test)]
pub(crate) mod stand_in {
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub struct Seen(pub Arc<Mutex<Vec<String>>>);

    impl Seen {
        pub fn all(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    pub async fn serve(answers: Vec<(&'static str, &'static str, u16, serde_json::Value)>) -> (String, Seen) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                // Read the head, then as much body as content-length says.
                loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(head_end) = text.find("\r\n\r\n") {
                        let len = text[..head_end]
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if buf.len() >= head_end + 4 + len {
                            break;
                        }
                    }
                }
                let req = String::from_utf8_lossy(&buf).to_string();
                let mut first = req.split_whitespace();
                let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
                log.lock().unwrap().push(req);
                let (code, body) = answers
                    .iter()
                    .find(|(m, p, _, _)| *m == method && *p == path)
                    .map(|(_, _, c, b)| (*c, b.to_string()))
                    .unwrap_or((404, "{}".into()));
                let reply = format!(
                    "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(reply.as_bytes()).await;
            }
        });
        (base, Seen(seen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_vendors_cookie_comes_out_one_shape() {
        let input = vec![
            // extension format, seconds
            json!({"domain": ".a.com", "name": "x", "value": "1", "path": "/", "secure": true,
                   "httpOnly": true, "sameSite": "no_restriction", "hostOnly": false,
                   "session": false, "expirationDate": 1893456000.5}),
            // milliseconds, as Dolphin's spec says and Multilogin's example shows
            json!({"domain": ".b.com", "name": "y", "value": "2", "expirationDate": 1893456000000u64}),
            // Undetectable: lowercase key, misspelt value, integer expiry
            json!({"domain": ".c.com", "name": "z", "value": "3", "samesite": "Unspecefied",
                   "expirationDate": 1893456000}),
            // Vision's spelling of SameSite=None
            json!({"domain": ".v.com", "name": "n", "value": "8", "same_site": "norestriction"}),
            // host-only must not become a domain cookie
            json!({"domain": "mail.d.com", "name": "h", "value": "4", "hostOnly": true,
                   "secure": true, "path": "/inbox"}),
            // session: no expiry, whatever the field says
            json!({"domain": ".e.com", "name": "s", "value": "5", "session": true, "expires": 1893456000}),
            // AdsPower: `expires`, a string
            json!({"domain": ".f.com", "name": "a", "value": "6", "expires": "1893456000"}),
            // nameless: dropped
            json!({"domain": ".g.com", "value": "7"}),
        ];
        let out = normalise_cookies(&input);
        assert_eq!(out.len(), 7);
        assert_eq!(out[0]["expires"], 1893456000.5);
        assert_eq!(out[0]["domain"], ".a.com");
        assert_eq!(out[1]["expires"], 1893456000.0);
        assert_eq!(out[2]["sameSite"], "Unspecefied");
        assert_eq!(out[3]["sameSite"], "no_restriction");
        assert!(out[4].get("domain").is_none());
        assert_eq!(out[4]["url"], "https://mail.d.com/inbox");
        assert!(out[5].get("expires").is_none());
        assert_eq!(out[6]["expires"], 1893456000.0);

        // and what cookies.rs then makes of them is CDP's own shape
        let cdp = crate::cookies::prepare(&out).unwrap();
        assert_eq!(cdp[0]["sameSite"], "None");
        assert!(cdp[2].get("sameSite").is_none(), "an unrecognised sameSite is dropped, not sent");
        assert_eq!(cdp[3]["sameSite"], "None");
    }

    #[test]
    fn a_proxy_line_is_one_the_paste_parser_reads_back() {
        let (line, note) = proxy_line("SOCKS5", "5.6.7.8", 1080, "u@x", "p:a%ss");
        assert_eq!(note, None);
        let p = fury_shared::proxy_list::parse_line(&line.unwrap()).unwrap();
        assert_eq!((p.host.as_str(), p.port), ("5.6.7.8", 1080));
        assert_eq!(p.username.as_deref(), Some("u@x"));
        assert_eq!(p.password.as_deref(), Some("p:a%ss"));
        assert!(proxy_line("ssh", "h", 22, "", "").0.is_none());
        assert!(proxy_line("http", "", 0, "", "").0.is_none());
    }

    #[test]
    fn os_words_are_the_shells() {
        for (raw, want) in [("Windows", "win"), ("win", "win"), ("macos", "mac"), ("MacOS", "mac"),
                            ("android", "android"), ("linux", "lin"), ("", "")] {
            assert_eq!(os_word(raw), want, "{raw}");
        }
    }
}
