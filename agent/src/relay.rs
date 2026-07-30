//! Per-profile local proxy relay.
//!
//! The browser is always launched with `--proxy-server=http://127.0.0.1:<port>`
//! and never talks to the upstream proxy directly. Five reasons, any one of
//! which would already require this component:
//!
//! 1. Chromium silently ignores credentials in `--proxy-server=socks5://u:p@h`.
//!    Authenticated SOCKS5 — which is most of the market — simply does not work.
//! 2. An authenticated HTTP proxy pops a credentials dialog that would otherwise
//!    have to be answered over CDP, on a profile that must stay free of
//!    automation traces.
//! 3. Upstream credentials must not reach the browser, or anyone with access to
//!    a profile walks away with the organisation's proxies.
//! 4. DNS. Without a relay Chromium may resolve names locally, and a site that
//!    sees a German exit IP resolving through a Russian resolver has learned
//!    something.
//! 5. It is the one place where a kill-switch can be enforced.
//!
//! # Kill-switch
//!
//! If the upstream is unreachable the relay returns an error to the browser and
//! closes. It never falls back to a direct connection. One request from the real
//! IP burns an account that took months to age, so "fail closed" is not
//! configurable.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone)]
pub enum Upstream {
    Http {
        host: String,
        port: u16,
        auth: Option<Credentials>,
    },
    /// Always SOCKS5h semantics: hostnames are sent to the proxy for remote
    /// resolution. Local resolution is never performed.
    Socks5 {
        host: String,
        port: u16,
        auth: Option<Credentials>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("upstream unreachable: {0}")]
    UpstreamUnreachable(#[source] io::Error),
    #[error("upstream refused authentication")]
    AuthRejected,
    #[error("upstream refused CONNECT to {target}: {reason}")]
    ConnectRejected { target: String, reason: String },
    #[error("malformed request from browser")]
    BadRequest,
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub struct Relay {
    upstream: Upstream,
}

impl Relay {
    pub fn new(upstream: Upstream) -> Self {
        Self { upstream }
    }

    /// Bind on loopback only and serve until the returned handle is dropped.
    /// Port 0 asks the OS for a free port, which is what the launcher uses.
    pub async fn serve(self, port: u16) -> anyhow::Result<(u16, tokio::task::JoinHandle<()>)> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await?;
        let bound = listener.local_addr()?.port();
        let this = Arc::new(self);

        let handle = tokio::spawn(async move {
            loop {
                let (client, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        continue;
                    }
                };
                let this = Arc::clone(&this);
                tokio::spawn(async move {
                    if let Err(e) = this.handle_client(client).await {
                        tracing::debug!(error = %e, "client session ended");
                    }
                });
            }
        });

        Ok((bound, handle))
    }

    async fn handle_client(&self, mut client: TcpStream) -> Result<(), RelayError> {
        let (head, leftover) = read_request_head(&mut client).await?;
        let (method, target) = parse_request_line(&head).ok_or(RelayError::BadRequest)?;

        // CONNECT is the HTTPS path and by far the common case.
        let (host, port) = if method == "CONNECT" {
            split_host_port(&target, 443).ok_or(RelayError::BadRequest)?
        } else {
            // Plain HTTP. Chromium sends an absolute URI to a proxy.
            absolute_uri_host(&target)
                .or_else(|| header_host(&head))
                .and_then(|h| split_host_port(&h, 80))
                .ok_or(RelayError::BadRequest)?
        };

        // Kill-switch lives here: a failure to reach upstream produces an error
        // response, never a direct connection.
        let mut upstream = match self.dial(&host, port).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(target = %host, error = %e, "upstream failed — refusing, not falling back");
                let _ = client
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n")
                    .await;
                return Err(e);
            }
        };

        if method == "CONNECT" {
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await?;
        } else {
            // Replay the request we already consumed, minus proxy-only headers.
            let cleaned = strip_proxy_headers(&head);
            upstream.write_all(cleaned.as_bytes()).await?;
            if !leftover.is_empty() {
                upstream.write_all(&leftover).await?;
            }
        }

        tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
        Ok(())
    }

    async fn dial(&self, host: &str, port: u16) -> Result<TcpStream, RelayError> {
        match &self.upstream {
            Upstream::Http {
                host: phost,
                port: pport,
                auth,
            } => {
                let mut s = TcpStream::connect((phost.as_str(), *pport))
                    .await
                    .map_err(RelayError::UpstreamUnreachable)?;
                http_connect(&mut s, host, port, auth.as_ref()).await?;
                Ok(s)
            }
            Upstream::Socks5 {
                host: phost,
                port: pport,
                auth,
            } => {
                let mut s = TcpStream::connect((phost.as_str(), *pport))
                    .await
                    .map_err(RelayError::UpstreamUnreachable)?;
                socks5_connect(&mut s, host, port, auth.as_ref()).await?;
                Ok(s)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SOCKS5 (RFC 1928 + RFC 1929)
// ---------------------------------------------------------------------------

async fn socks5_connect(
    s: &mut TcpStream,
    host: &str,
    port: u16,
    auth: Option<&Credentials>,
) -> Result<(), RelayError> {
    // Greeting. Offer no-auth and username/password.
    let methods: &[u8] = if auth.is_some() { &[0x00, 0x02] } else { &[0x00] };
    let mut greeting = vec![0x05, methods.len() as u8];
    greeting.extend_from_slice(methods);
    s.write_all(&greeting).await?;

    let mut resp = [0u8; 2];
    s.read_exact(&mut resp).await?;
    if resp[0] != 0x05 {
        return Err(RelayError::AuthRejected);
    }

    match resp[1] {
        0x00 => {}
        0x02 => {
            let c = auth.ok_or(RelayError::AuthRejected)?;
            let mut req = vec![0x01, c.username.len() as u8];
            req.extend_from_slice(c.username.as_bytes());
            req.push(c.password.len() as u8);
            req.extend_from_slice(c.password.as_bytes());
            s.write_all(&req).await?;

            let mut ar = [0u8; 2];
            s.read_exact(&mut ar).await?;
            if ar[1] != 0x00 {
                return Err(RelayError::AuthRejected);
            }
        }
        _ => return Err(RelayError::AuthRejected),
    }

    // CONNECT with ATYP=domain. Sending the hostname rather than an address is
    // what makes this SOCKS5h: the proxy resolves, so DNS never leaks locally.
    let host_bytes = host.as_bytes();
    if host_bytes.len() > 255 {
        return Err(RelayError::BadRequest);
    }
    let mut req = vec![0x05, 0x01, 0x00, 0x03, host_bytes.len() as u8];
    req.extend_from_slice(host_bytes);
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await?;

    let mut head = [0u8; 4];
    s.read_exact(&mut head).await?;
    if head[1] != 0x00 {
        return Err(RelayError::ConnectRejected {
            target: format!("{host}:{port}"),
            reason: socks5_reply_reason(head[1]).to_string(),
        });
    }

    // Consume the bound address so the stream is positioned at payload.
    match head[3] {
        0x01 => {
            let mut skip = [0u8; 4 + 2];
            s.read_exact(&mut skip).await?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await?;
            let mut skip = vec![0u8; len[0] as usize + 2];
            s.read_exact(&mut skip).await?;
        }
        0x04 => {
            let mut skip = [0u8; 16 + 2];
            s.read_exact(&mut skip).await?;
        }
        _ => return Err(RelayError::BadRequest),
    }

    Ok(())
}

fn socks5_reply_reason(code: u8) -> &'static str {
    match code {
        0x01 => "general SOCKS server failure",
        0x02 => "connection not allowed by ruleset",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "TTL expired",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------------
// HTTP CONNECT
// ---------------------------------------------------------------------------

async fn http_connect(
    s: &mut TcpStream,
    host: &str,
    port: u16,
    auth: Option<&Credentials>,
) -> Result<(), RelayError> {
    let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if let Some(c) = auth {
        let token = base64(format!("{}:{}", c.username, c.password).as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {token}\r\n"));
    }
    req.push_str("Proxy-Connection: Keep-Alive\r\n\r\n");
    s.write_all(req.as_bytes()).await?;

    let (head, _) = read_request_head(s).await?;
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);

    match status {
        200..=299 => Ok(()),
        407 => Err(RelayError::AuthRejected),
        _ => Err(RelayError::ConnectRejected {
            target: format!("{host}:{port}"),
            reason: format!("HTTP {status}"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Read until the end of headers. Returns the head as text plus any body bytes
/// that arrived in the same read.
async fn read_request_head(s: &mut TcpStream) -> Result<(String, Vec<u8>), RelayError> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(RelayError::BadRequest);
        }
        buf.extend_from_slice(&chunk[..n]);

        if let Some(pos) = find_headers_end(&buf) {
            let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
            let leftover = buf[pos..].to_vec();
            return Ok((head, leftover));
        }
        // A head this large is not a browser talking to its own proxy.
        if buf.len() > 64 * 1024 {
            return Err(RelayError::BadRequest);
        }
    }
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn parse_request_line(head: &str) -> Option<(String, String)> {
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    Some((method, target))
}

fn split_host_port(s: &str, default_port: u16) -> Option<(String, u16)> {
    // IPv6 literal.
    if let Some(rest) = s.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = tail
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(default_port);
        return Some((host.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => Some((h.to_string(), p.parse().ok()?)),
        _ => Some((s.to_string(), default_port)),
    }
}

fn absolute_uri_host(target: &str) -> Option<String> {
    let rest = target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))?;
    Some(rest.split('/').next()?.to_string())
}

fn header_host(head: &str) -> Option<String> {
    head.lines()
        .find(|l| l.to_ascii_lowercase().starts_with("host:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}

/// Remove hop-by-hop headers that must not reach the origin server, and rewrite
/// the absolute-form request line back to origin form.
fn strip_proxy_headers(head: &str) -> String {
    let mut out = String::with_capacity(head.len());
    for (i, line) in head.lines().enumerate() {
        if i == 0 {
            let mut parts = line.split_whitespace();
            let (m, t, v) = (
                parts.next().unwrap_or("GET"),
                parts.next().unwrap_or("/"),
                parts.next().unwrap_or("HTTP/1.1"),
            );
            let path = t
                .strip_prefix("http://")
                .or_else(|| t.strip_prefix("https://"))
                .and_then(|r| r.find('/').map(|i| &r[i..]))
                .unwrap_or(if t.starts_with("http") { "/" } else { t });
            out.push_str(&format!("{m} {path} {v}\r\n"));
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("proxy-connection:") || lower.starts_with("proxy-authorization:") {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for c in input.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_connect_target() {
        let (m, t) = parse_request_line("CONNECT example.com:443 HTTP/1.1\r\n").unwrap();
        assert_eq!(m, "CONNECT");
        assert_eq!(split_host_port(&t, 443).unwrap(), ("example.com".into(), 443));
    }

    #[test]
    fn defaults_port_when_absent() {
        assert_eq!(split_host_port("example.com", 443).unwrap().1, 443);
    }

    #[test]
    fn handles_ipv6_literals() {
        let (h, p) = split_host_port("[2606:4700::1111]:8443", 443).unwrap();
        assert_eq!(h, "2606:4700::1111");
        assert_eq!(p, 8443);
    }

    #[test]
    fn rewrites_absolute_uri_to_origin_form() {
        let head = "GET http://example.com/a?b=1 HTTP/1.1\r\n\
                    Host: example.com\r\n\
                    Proxy-Connection: Keep-Alive\r\n\
                    Proxy-Authorization: Basic zzz\r\n\
                    Accept: */*\r\n\r\n";
        let out = strip_proxy_headers(head);
        assert!(out.starts_with("GET /a?b=1 HTTP/1.1\r\n"));
        assert!(!out.to_lowercase().contains("proxy-authorization"));
        assert!(!out.to_lowercase().contains("proxy-connection"));
        assert!(out.contains("Accept: */*"));
    }

    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }

    #[test]
    fn finds_end_of_headers() {
        assert_eq!(find_headers_end(b"GET / HTTP/1.1\r\n\r\nbody"), Some(18));
        assert_eq!(find_headers_end(b"GET / HTTP/1.1\r\n"), None);
    }
}
