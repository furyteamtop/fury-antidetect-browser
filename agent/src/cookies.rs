// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Cookie import and export, over CDP.
//!
//! # Why not the SQLite file
//!
//! The obvious implementation is to open `Default/Cookies` and read it. It does
//! not work, and the way it fails is expensive.
//!
//! Since schema v24 a cookie's plaintext is `SHA256(host_key) || value`,
//! encrypted under a key from the OS: the "Chromium Safe Storage" keychain item
//! on macOS, DPAPI or an app-bound key on Windows. The Windows app-bound key
//! ("v20") can only be unwrapped by a COM call into an elevation service running
//! as SYSTEM, so on Windows there is no general way to read a cookie value
//! outside the browser at all.
//!
//! Writing is worse than reading. `LoadCookiesForDomains`
//! (net/extras/sqlite/sqlite_persistent_cookie_store.cc:916-935) does not skip a
//! row it cannot decrypt — it runs `DELETE FROM cookies WHERE host_key = ?` for
//! **every host in that eTLD+1 group**, "for data consistency". A single
//! malformed row costs the whole site's session.
//!
//! # So: CDP, on the browser endpoint
//!
//! `Storage.getCookies` and `Storage.setCookies` are registered on the browser
//! target (browser_devtools_agent_host.cc), which is the endpoint
//! `devtools_endpoint()` already returns, and they cover every cookie in the
//! profile rather than one tab's. The browser does its own encryption on the way
//! in and its own decryption on the way out, so nothing here needs a key and
//! nothing here can corrupt the jar.
//!
//! The format is CDP's own `Network.Cookie` / `Network.CookieParam` JSON. One
//! format, both directions, and it is the wire format of the thing doing the
//! writing — any other choice would need a translation table between Fury's idea
//! of a cookie and Chromium's, which is a second source of truth for one fact.

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// A CDP connection, alive for one exchange.
///
/// Deliberately not pooled or kept: a socket held open across a launch would
/// outlive the browser it points at, and the failure would land later as a
/// timeout with nothing pointing back here.
pub struct Cdp {
    socket: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    next_id: u64,
}

impl Cdp {
    pub async fn connect(ws_endpoint: &str) -> anyhow::Result<Self> {
        // Loopback only, and worth stating: the endpoint comes from
        // DevToolsActivePort in our own profile directory, not from anything a
        // page said. Anyone who can reach this port can read every cookie, which
        // is why `Perm::ExportCookies` gates CDP in the first place.
        if !ws_endpoint.starts_with("ws://127.0.0.1:") && !ws_endpoint.starts_with("ws://localhost:")
        {
            anyhow::bail!("refusing to open a CDP connection to {ws_endpoint:?}: not loopback");
        }
        let (socket, _) = tokio_tungstenite::connect_async(ws_endpoint).await?;
        Ok(Self { socket, next_id: 1 })
    }

    /// One command, one reply.
    ///
    /// CDP interleaves events with replies on the same socket, so this skips
    /// anything without our id rather than taking the next frame — reading the
    /// next frame is how a client ends up returning a `Network.requestWillBeSent`
    /// where a cookie list was expected.
    pub async fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({ "id": id, "method": method, "params": params });
        self.socket.send(Message::Text(request.to_string())).await?;

        loop {
            let Some(frame) = self.socket.next().await else {
                anyhow::bail!("the browser closed the CDP connection before answering {method}");
            };
            let text = match frame? {
                Message::Text(t) => t,
                Message::Close(_) => {
                    anyhow::bail!("the browser closed the CDP connection during {method}")
                }
                // Ping/Pong are handled by the library; anything else is not a
                // CDP message and is not ours to interpret.
                _ => continue,
            };
            let value: serde_json::Value = serde_json::from_str(&text)?;
            if value.get("id").and_then(|v| v.as_u64()) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                anyhow::bail!("{method} failed: {error}");
            }
            return Ok(value.get("result").cloned().unwrap_or(serde_json::Value::Null));
        }
    }
}

/// Every cookie in the profile, in CDP's `Network.Cookie` shape.
pub async fn export(ws_endpoint: &str) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut cdp = Cdp::connect(ws_endpoint).await?;
    // No browserContextId: the default context is the profile's own jar, which
    // is the one that was loaded from disk and the one that gets written back.
    let result = cdp.call("Storage.getCookies", serde_json::json!({})).await?;
    Ok(result
        .get("cookies")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default())
}

/// Add cookies to the profile.
///
/// Additive, not replacing. Clearing first would make a failed import
/// destructive, and the case that matters — seeding a fresh profile — has
/// nothing to clear.
pub async fn import(ws_endpoint: &str, cookies: &[serde_json::Value]) -> anyhow::Result<Imported> {
    let prepared = prepare(cookies)?;
    if prepared.is_empty() {
        anyhow::bail!("nothing to import: no line carried both a name and a domain");
    }
    let accepted = prepared.len();
    // A cookie with no expiry is a session cookie, and Chromium does not write
    // those to disk — so when this import opened the profile itself, they are
    // gone the moment it closes again. Measured: two cookies in, one out.
    //
    // That is the browser behaving correctly, and it is still a surprise worth
    // naming. Someone importing a warmed account from a dump full of session
    // cookies would otherwise see "imported: 40" and find eleven.
    let session_only = prepared
        .iter()
        .filter(|c| c.get("expires").is_none())
        .count();
    let skipped = cookies.len().saturating_sub(accepted);

    let mut cdp = Cdp::connect(ws_endpoint).await?;
    cdp.call("Storage.setCookies", serde_json::json!({ "cookies": prepared }))
        .await?;
    Ok(Imported { accepted, session_only, skipped })
}

/// What an import did, in enough detail to explain a number that looks wrong.
pub struct Imported {
    pub accepted: usize,
    /// Of those, how many have no expiry and will not survive the profile
    /// closing.
    pub session_only: usize,
    /// Entries that carried no name, or neither a domain nor a url.
    pub skipped: usize,
}

/// Turn what people actually have into what `Storage.setCookies` accepts.
///
/// The three shapes in circulation are CDP's own, Puppeteer's `page.cookies()`
/// (the same field names — it is CDP underneath), and the array the cookie-editor
/// extensions emit, which differs in exactly two places: `expirationDate`
/// instead of `expires`, and `hostOnly`/`session` booleans Chromium infers for
/// itself.
///
/// Fields Chromium does not know are dropped rather than passed through:
/// `setCookies` rejects the whole batch on an unknown key, so forwarding one
/// would turn a working export from another tool into a total failure.
fn prepare(cookies: &[serde_json::Value]) -> anyhow::Result<Vec<serde_json::Value>> {
    const KNOWN: &[&str] = &[
        "name", "value", "url", "domain", "path", "secure", "httpOnly", "sameSite", "expires",
        "priority", "sameParty", "sourceScheme", "sourcePort", "partitionKey",
    ];

    let mut out = Vec::with_capacity(cookies.len());
    for raw in cookies {
        let Some(object) = raw.as_object() else { continue };

        let name = object.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let domain = object.get("domain").and_then(|v| v.as_str()).unwrap_or("");
        let has_url = object.get("url").and_then(|v| v.as_str()).is_some_and(|u| !u.is_empty());
        // A cookie with no name, or with neither a domain nor a url, has nowhere
        // to go. Skipped rather than refused: a hand-edited file with one bad
        // line should still import the other four hundred.
        if name.is_empty() || (domain.is_empty() && !has_url) {
            continue;
        }

        let mut cookie = serde_json::Map::new();
        for (key, value) in object {
            if KNOWN.contains(&key.as_str()) {
                cookie.insert(key.clone(), value.clone());
            }
        }

        // Cookie-editor's field name. Seconds since the epoch either way, but
        // often a float, and CDP wants a number — so it passes through as one.
        if !cookie.contains_key("expires") {
            if let Some(expiry) = object
                .get("expirationDate")
                .or_else(|| object.get("expiry"))
                .and_then(|v| v.as_f64())
            {
                cookie.insert("expires".into(), serde_json::json!(expiry));
            }
        }

        // A session cookie is one with no expiry, which is how CDP already reads
        // an absent field — so `session: true` needs nothing but removing, and
        // it is not in KNOWN.

        // sameSite arrives lowercase from several exporters and CDP wants
        // "Strict" | "Lax" | "None". A value it does not recognise fails the
        // whole batch, so an unrecognisable one is dropped instead.
        if let Some(same_site) = cookie.get("sameSite").and_then(|v| v.as_str()) {
            let normalised = match same_site.to_ascii_lowercase().as_str() {
                "strict" => Some("Strict"),
                "lax" => Some("Lax"),
                "none" | "no_restriction" => Some("None"),
                // Cookie-editor writes this for "not set", which is not a value.
                _ => None,
            };
            match normalised {
                Some(v) => {
                    cookie.insert("sameSite".into(), serde_json::json!(v));
                }
                None => {
                    cookie.remove("sameSite");
                }
            }
        }

        out.push(serde_json::Value::Object(cookie));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cookie_editor_export_becomes_something_cdp_accepts() {
        // The shape those extensions actually emit, unknown fields and all.
        let input = serde_json::json!([{
            "name": "sid",
            "value": "abc",
            "domain": ".example.com",
            "path": "/",
            "secure": true,
            "httpOnly": true,
            "sameSite": "no_restriction",
            "expirationDate": 1893456000.5,
            "hostOnly": false,
            "session": false,
            "storeId": "0",
            "id": 1
        }]);
        let out = prepare(input.as_array().unwrap()).unwrap();
        assert_eq!(out.len(), 1);
        let c = &out[0];
        assert_eq!(c["name"], "sid");
        assert_eq!(c["sameSite"], "None");
        assert_eq!(c["expires"], 1893456000.5);
        // Anything CDP does not know fails the WHOLE batch, so passing one
        // through would turn a working export from another tool into a total
        // failure rather than a partial one.
        for unknown in ["hostOnly", "session", "storeId", "id", "expirationDate"] {
            assert!(c.get(unknown).is_none(), "{unknown} should have been dropped");
        }
    }

    #[test]
    fn a_cdp_export_survives_a_round_trip_unchanged() {
        // Export then import must be the identity, or moving a profile between
        // machines quietly loses fields.
        let input = serde_json::json!([{
            "name": "sid", "value": "abc", "domain": ".example.com", "path": "/",
            "expires": 1893456000.0, "size": 12, "httpOnly": true, "secure": true,
            "session": false, "sameSite": "Lax", "priority": "Medium",
            "sameParty": false, "sourceScheme": "Secure", "sourcePort": 443
        }]);
        let out = prepare(input.as_array().unwrap()).unwrap();
        let c = &out[0];
        assert_eq!(c["sameSite"], "Lax");
        assert_eq!(c["sourcePort"], 443);
        assert_eq!(c["priority"], "Medium");
        // `size` is a read-only field CDP reports and refuses to be given.
        assert!(c.get("size").is_none());
    }

    #[test]
    fn a_line_with_nowhere_to_go_is_skipped_and_the_rest_import() {
        let input = serde_json::json!([
            { "name": "", "value": "x", "domain": "example.com" },
            { "name": "orphan", "value": "x" },
            { "name": "good", "value": "x", "domain": "example.com" },
            { "name": "by-url", "value": "x", "url": "https://example.com/" },
            "not an object",
        ]);
        let out = prepare(input.as_array().unwrap()).unwrap();
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0]["name"], "good");
        assert_eq!(out[1]["name"], "by-url");
    }

    #[tokio::test]
    async fn cdp_refuses_an_endpoint_that_is_not_loopback() {
        // The endpoint comes from our own profile directory, so this can only
        // fire if something upstream started taking it from elsewhere — which
        // is exactly when handing over every cookie would matter.
        assert!(Cdp::connect("ws://evil.example.com:9222/devtools/browser/x")
            .await
            .is_err());
        assert!(Cdp::connect("wss://127.0.0.1.evil.com:9222/x").await.is_err());
    }
}
