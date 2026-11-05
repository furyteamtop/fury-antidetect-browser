// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Fetching a `.crx` from the Chrome Web Store by id, the way the browser
//! itself would — and from where the browser would.
//!
//! The update endpoint hands out the current package for any id. What matters
//! here is not the URL but the route: the download goes **through the proxy
//! of the profile it is being installed into**, so Google sees the exit that
//! profile always shows it, and never the operator's own address beside an
//! extension id that says what the operator is up to. That was the objection
//! in `install_core.rs` to fetching anything automatically, and the decision
//! recorded in docs/12 (B, 12.09.2026) is this compromise: fetch, but only
//! along the profile's own path. A profile with no proxy fetches directly,
//! because directly *is* its path.
//!
//! What comes back is checked, not trusted: `ext::parse` recovers the signing
//! key and derives the id from it, and the caller refuses a package whose key
//! does not derive the id that was asked for. The endpoint is Google's, over
//! TLS, but the check costs nothing and is what "installed by id" means.

use anyhow::{bail, Context, Result};

/// A package larger than this is not an extension somebody would put in a
/// profile; it is a mistake or a redirect to somewhere else.
const MAX_BYTES: usize = 128 * 1024 * 1024;

/// The same request Chromium makes when it installs from the store.
pub fn crx_url(id: &str) -> String {
    format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion={}&acceptformat=crx3&x=id%3D{id}%26uc",
        crate::CHROME_FULL_VERSION
    )
}

/// Download the package for `id`, through `proxy_url` when there is one.
pub async fn fetch(id: &str, proxy_url: Option<&str>) -> Result<Vec<u8>> {
    if !fury_shared::extensions::is_id(id) {
        bail!("{id:?} is not an extension id");
    }
    let mut b = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .user_agent(format!(
            "Mozilla/5.0 AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{} Safari/537.36",
            crate::CHROME_FULL_VERSION
        ));
    // Through the relay the browser itself uses, never the proxy's URL handed
    // to reqwest. That URL is the login as typed, not percent-encoded, so a
    // password holding `#`, `/` or `?` came back as "invalid port number" here
    // while the same proxy opened the same profile (tester, 04.11.2026: the
    // store and the catalogue always failed, a .crx file never did). It also
    // resolved names on this machine for socks5://, which the relay does not.
    // Held until the download is done: the relay stops when this goes.
    let mut _relay = None;
    if let Some(p) = proxy_url {
        let (addr, guard) = crate::relay::loopback(crate::parse_upstream(p)?).await?;
        _relay = Some(guard);
        b = b.proxy(reqwest::Proxy::all(&addr).context("the profile's proxy address")?);
    }
    let url = crx_url(id);
    let res = match b.build()?.get(&url).send().await {
        Ok(res) => res,
        Err(e) => {
            // The relay answers a failed upstream with a bare 502, which reads
            // as Google's fault. Ask again for the cause in the relay's words.
            if let Some(p) = proxy_url {
                if let Some(why) = crate::relay::explain(crate::parse_upstream(p)?, &url).await {
                    bail!("the profile's proxy: {why}");
                }
            }
            return Err(e).context("reaching the Web Store");
        }
    };
    if !res.status().is_success() {
        // 204 is what the endpoint says for an id it has never heard of.
        bail!("the Web Store answered {} for {id}", res.status());
    }
    if let Some(len) = res.content_length() {
        if len as usize > MAX_BYTES {
            bail!("the package is {len} bytes, which is not an extension");
        }
    }
    let bytes = res.bytes().await.context("downloading the package")?;
    if bytes.len() > MAX_BYTES {
        bail!("the package is {} bytes, which is not an extension", bytes.len());
    }
    if bytes.len() < 16 || &bytes[..4] != b"Cr24" {
        bail!("the Web Store did not return a CRX for {id}");
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_url_carries_the_id_and_our_version() {
        let u = super::crx_url("ddkjiahejlhfcafbddmgiahcphecmpfh");
        assert!(u.contains("x=id%3Dddkjiahejlhfcafbddmgiahcphecmpfh%26uc"));
        assert!(u.contains(&format!("prodversion={}", crate::CHROME_FULL_VERSION)));
        assert!(u.starts_with("https://clients2.google.com/"));
    }

    /// A login with characters a URL gives a meaning to reaches the proxy as it
    /// was typed. Until 0.5.0 it never left this process.
    #[tokio::test]
    async fn a_password_with_url_characters_reaches_the_proxy_as_typed() {
        use base64::Engine as _;
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = tokio::spawn(async move {
            let (mut c, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            let mut buf = [0u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = c.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                head.extend_from_slice(&buf[..n]);
            }
            c.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n").await.ok();
            String::from_utf8_lossy(&head).to_string()
        });

        let id = "a".repeat(32);
        let err = super::fetch(&id, Some(&format!("http://user:p#a/b?c@127.0.0.1:{port}"))).await.unwrap_err();
        let head = seen.await.unwrap();
        let line = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("proxy-authorization"))
            .unwrap_or_else(|| panic!("no login sent: {head}"));
        let sent = base64::engine::general_purpose::STANDARD
            .decode(line.rsplit(' ').next().unwrap().trim())
            .unwrap();
        assert_eq!(String::from_utf8(sent).unwrap(), "user:p#a/b?c");
        assert!(!format!("{err:#}").contains("invalid port"), "{err:#}");
    }
}
