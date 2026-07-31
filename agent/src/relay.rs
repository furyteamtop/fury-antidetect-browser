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

#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// Derived `Debug` printed the password. Three `tracing::info!(?upstream, …)`
/// call sites — one on every proxy check and one on every launch — put a
/// customer's proxy password into the agent log in cleartext, where it outlives
/// the session, gets copied into bug reports, and is read by anything that
/// ships logs anywhere.
///
/// The username stays: telling two accounts on one provider apart is the reason
/// to look at this at all.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"…")
            .finish()
    }
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
    /// How to name this profile's exit on the start page.
    ///
    /// Host and port only — never the credentials. The page is rendered inside
    /// the profile's own browser, where any site it later visits could read the
    /// document if something went wrong; a proxy password does not belong
    /// anywhere near that.
    fn upstream_label(&self) -> String {
        let (host, port, scheme) = match &self.upstream {
            Upstream::Http { host, port, .. } => (host, port, "http"),
            Upstream::Socks5 { host, port, .. } => (host, port, "socks5"),
        };
        format!("{scheme}://{host}:{port}")
    }

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

        // The start page is answered here rather than fetched from anywhere.
        //
        // AdsPower opens a page on its own servers at launch, which tells the
        // vendor every time an operator starts a profile and from which exit.
        // Serving it from the relay keeps that entirely local: the check that
        // the proxy works happens inside the same component that provides the
        // proxy, and nothing about the launch leaves the machine.
        if method != "CONNECT" && is_start_page(&target, &head) {
            let body = start_page(&self.upstream_label());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                body.len()
            );
            client.write_all(response.as_bytes()).await?;
            client.write_all(body.as_bytes()).await?;
            return Ok(());
        }

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

/// Minimal HTML escaping for the one value the start page interpolates.
fn escape_html(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Is this a request for the profile's own start page?
fn is_start_page(target: &str, head: &str) -> bool {
    let host = absolute_uri_host(target)
        .or_else(|| header_host(head))
        .unwrap_or_default();
    host.split(':').next().unwrap_or_default() == START_HOST
}

/// The hostname the start page answers on.
///
/// A `.invalid` name by design (RFC 6761): it can never resolve in DNS, so if
/// this interception ever failed to fire, the request would fail loudly instead
/// of quietly reaching a real site of that name.
pub const START_HOST: &str = "fury.invalid";
pub const START_URL: &str = "http://fury.invalid/";

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

/// The page a profile opens on.
///
/// Everything it reports is measured in the page itself, by the browser being
/// tested — which is the only way to check a disguise: what the operator needs
/// to see is what a site would see, not what the configuration intended.
fn start_page(exit: &str) -> String {
    // Escaped here rather than by the caller. The function that interpolates is
    // the one that has to be safe: a caller that forgets is a bug nobody sees
    // until a proxy hostname contains a bracket.
    let exit = escape_html(exit);
    format!(
        r##"<!doctype html><meta charset="utf-8"><title>Fury</title>
<style>
 :root{{color-scheme:dark}}
 body{{margin:0;background:#0e1013;color:#e8eaed;
   font:14px/1.6 -apple-system,system-ui,sans-serif;display:grid;place-items:center;min-height:100vh}}
 main{{width:min(620px,92vw);padding:28px 0}}
 h1{{margin:0 0 4px;font-size:20px;color:#e0552f}}
 p.sub{{margin:0 0 24px;color:#9aa3b0;font-size:13px}}
 dl{{display:grid;grid-template-columns:150px 1fr;gap:8px 16px;margin:0 0 20px;font-size:13px}}
 dt{{color:#6b7480}} dd{{margin:0;overflow-wrap:anywhere}}
 /* Deliberately not a monospace stack. The font filter narrows the list to the
    persona's fonts, so a Windows profile on a Mac has no monospace face that
    actually exists on the host — and text asking for one renders as nothing at
    all. That is a real defect in the filter, recorded in docs/09; this page
    must not be the thing that hides it. */
 code{{font-family:inherit;font-size:12.5px;color:#e8eaed}}
 .v{{padding:10px 14px;border-radius:8px;font-size:13px}}
 .ok{{background:rgba(76,175,125,.1);color:#4caf7d}}
 .bad{{background:rgba(224,160,48,.1);color:#e0a030}}
</style>
<main>
 <h1>Fury</h1>
 <p class="sub">What this profile reports right now, measured in this page. Nothing here left your machine.</p>
 <dl id="facts"></dl>
 <div id="verdict" class="v"></div>
 <p class="sub" style="margin-top:20px">Exit: <code>{exit}</code></p>
</main>
<script>
const g=(f)=>{{try{{return f()}}catch(e){{return "—"}}}};
const gl=g(()=>{{const c=document.createElement("canvas").getContext("webgl");
  const d=c.getExtension("WEBGL_debug_renderer_info");
  return c.getParameter(d.UNMASKED_RENDERER_WEBGL)}});
const facts={{
 "Platform":navigator.platform,
 "User agent":navigator.userAgent,
 "Languages":navigator.languages.join(", "),
 "Time zone":Intl.DateTimeFormat().resolvedOptions().timeZone,
 "Screen":screen.width+"×"+screen.height+" (usable "+screen.availWidth+"×"+screen.availHeight+")",
 "GPU":gl,
 "CPU · RAM":navigator.hardwareConcurrency+" cores · "+(navigator.deviceMemory||"?")+" GB",
 "Touch points":navigator.maxTouchPoints,
 "navigator.webdriver":String(navigator.webdriver),
}};
const dl=document.getElementById("facts");
for(const[k,v]of Object.entries(facts)){{
 dl.insertAdjacentHTML("beforeend","<dt></dt><dd><code></code></dd>");
 dl.children[dl.children.length-2].textContent=k;
 dl.lastElementChild.firstChild.textContent=v;
}}
// The checks a site would actually run: does the platform agree with the user
// agent, and does the GPU belong to the operating system being claimed.
const ua=navigator.userAgent, mac=/Mac OS X/.test(ua), win=/Windows NT/.test(ua);
const problems=[];
if(mac&&navigator.platform!=="MacIntel")problems.push("macOS user agent with platform "+navigator.platform);
if(win&&navigator.platform!=="Win32")problems.push("Windows user agent with platform "+navigator.platform);
if(mac&&/NVIDIA|Direct3D/.test(gl))problems.push("macOS user agent with a Windows GPU string");
if(win&&/Metal|Apple/.test(gl))problems.push("Windows user agent with an Apple GPU string");
if(navigator.webdriver)problems.push("navigator.webdriver is true");
const v=document.getElementById("verdict");
v.className="v "+(problems.length?"bad":"ok");
v.textContent=problems.length?problems.join(" · "):"Consistent — the platform, the user agent and the GPU agree.";
</script>"##
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_password_never_reaches_a_log_line() {
        // Every launch and every proxy check logs the upstream at INFO. The
        // derived Debug put the password in that line.
        let up = Upstream::Socks5 {
            host: "exit.example".into(),
            port: 1080,
            auth: Some(Credentials {
                username: "bob".into(),
                password: "s3cr3t-proxy-password".into(),
            }),
        };
        let printed = format!("{up:?}");
        assert!(!printed.contains("s3cr3t"), "the password is in the log: {printed}");
        assert!(printed.contains("bob"), "the username is what tells two accounts apart");
    }

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
    fn the_start_page_is_recognised_however_it_is_addressed() {
        assert!(is_start_page("http://fury.invalid/", ""));
        assert!(is_start_page("/", "Host: fury.invalid\r\n"));
        assert!(is_start_page("http://fury.invalid:80/x", ""));
        // And nothing else is intercepted, or a real site could be shadowed.
        assert!(!is_start_page("http://example.com/", ""));
        assert!(!is_start_page("http://notfury.invalid/", ""));
    }

    #[test]
    fn the_start_page_escapes_what_it_prints() {
        // The exit label comes from a proxy hostname the operator typed. It is
        // shown, so it must not be able to close the tag it sits in.
        let page = start_page("<script>alert(1)</script>");
        assert!(!page.contains("<script>alert(1)"), "exit label was not escaped");
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
