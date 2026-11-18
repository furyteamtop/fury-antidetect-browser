// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

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
    /// An HTTP proxy, spoken to with CONNECT.
    ///
    /// `tls` is an `https://` address: the CONNECT goes inside a TLS session
    /// with the proxy, the way `curl --proxy https://` sends it. Until 0.5.7
    /// `https://` was dialled as plain TCP, and a real HTTPS proxy closed the
    /// connection on the first plaintext byte: every page in the profile was a
    /// 502 (issue 37). A proxy sold as "HTTPS" that is really plain HTTP with
    /// CONNECT now fails instead, with a sentence that says to address it as
    /// `http://` (`tls_to_proxy`), and the proxy check offers the switch
    /// (`diagnose::speaks_instead`). Never a silent fallback to plaintext: the
    /// operator chose an encrypted hop to the proxy, and quietly sending the
    /// password in clear would undo that choice behind their back.
    Http {
        host: String,
        port: u16,
        auth: Option<Credentials>,
        tls: bool,
    },
    /// Always SOCKS5h semantics: hostnames are sent to the proxy for remote
    /// resolution. Local resolution is never performed.
    Socks5 {
        host: String,
        port: u16,
        auth: Option<Credentials>,
    },
    /// No proxy at all: the relay dials out from this machine, and the profile
    /// shows every site the address this machine already has.
    ///
    /// Off unless a profile says otherwise in as many words, and never for a
    /// team profile — one of those opens on a colleague's machine, where
    /// "this machine" is somebody who did not choose it. See `allow_no_proxy`
    /// on the profile.
    ///
    /// The relay still stands in the middle, and it is not ceremony: the
    /// blocklist, the start page and the refusal to reach this machine's own
    /// services all live here, and a browser pointed straight at the network
    /// would have none of them.
    ///
    /// There is nothing to fail closed about — the exit is the machine — so
    /// the kill-switch simply has no work in this mode.
    Direct,

    /// A WireGuard peer: the tunnel (shared with anything else in the process
    /// using the same config, see `wg_stack::Shared`) and the OS this
    /// profile's connections are to look like inside it (`syn_shape`).
    ///
    /// Unlike the other two this is not an address to connect to — it is a
    /// network the agent is already inside. Names are resolved by the peer's
    /// resolvers through the tunnel (`wg_stack::Stack::resolve`), for the same
    /// reason SOCKS5 is used with SOCKS5h semantics: asking this machine's
    /// resolver what the profile is about to visit is the leak, whatever
    /// carries the bytes afterwards.
    WireGuard(std::sync::Arc<crate::wg_stack::Shared>, crate::syn_shape::Os),
}

/// How long to wait for the proxy's TCP handshake.
///
/// There was no limit here at all until an HTTP proxy addressed as `socks5://`
/// made every tab in a profile hang forever instead of failing. `diagnose`
/// had its own bound and the launch path did not, so the one place an operator
/// actually meets a broken proxy was the one place that never gave up.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long to wait for the proxy to answer its own protocol's greeting, once
/// the socket is open.
///
/// Shorter than the connect, and deliberately: a SOCKS5 server replies to a
/// three-byte greeting within one round trip, and an HTTP proxy that received
/// that greeting is sitting there waiting for a request line that will never
/// come. Silence here is evidence about what the far end is, which is why the
/// timeout produces [`RelayError::WrongProtocol`] rather than "unreachable".
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("upstream unreachable: {0}")]
    UpstreamUnreachable(#[source] io::Error),
    #[error("upstream refused authentication")]
    AuthRejected,
    /// A SOCKS5 server offered a login and password that answered "no
    /// acceptable methods": it takes neither, which usually means access is by
    /// the caller's IP address. Not the same advice as a wrong password.
    #[error("upstream accepts no login and password")]
    NoLoginAccepted,
    /// The socket opened and then the far end did not behave like the protocol
    /// its address claims.
    ///
    /// Worth its own variant because the fix is a one-click change of a
    /// dropdown, and because the alternatives lie: reported as "unreachable" it
    /// sends somebody to their firewall, and reported as "authentication
    /// refused" — which is what a non-0x05 greeting byte used to produce — it
    /// sends them to their provider to reset a password that was never wrong.
    #[error("not a {expected} proxy: {saw}")]
    WrongProtocol { expected: &'static str, saw: String },
    #[error("upstream refused CONNECT to {target}: {reason}")]
    ConnectRejected { target: String, reason: String },
    /// The proxy speaks TLS, but its certificate failed the check against this
    /// machine's trusted roots, which it gets like any site's. Not skipped for
    /// a proxy: whoever can present a certificate for it unchecked reads the
    /// proxy password and every destination the profile opens.
    #[error("the proxy's TLS certificate was refused: {0}")]
    ProxyCertificate(String),
    /// TLS with the proxy failed for any other reason.
    #[error("TLS with the proxy failed: {0}")]
    ProxyTls(String),
    #[error("malformed request from browser")]
    BadRequest,
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub struct Relay {
    upstream: Upstream,
    /// Hosts this profile refuses to talk to. Empty for most profiles, and
    /// then it costs one `is_empty` per connection.
    blocked: std::sync::Arc<crate::blocklist::Blocklist>,
    /// The secret in the start page's path. See `is_ours`.
    token: String,
    /// Set once by `serve`, read by `is_ours`: the port the probe answers on.
    port: std::sync::OnceLock<u16>,
    /// Lets a check reach a checker on the operator's own network. Never set
    /// for a profile; see `is_local_target` in `handle_client`.
    allow_local: bool,
}

impl Relay {
    pub fn new(upstream: Upstream) -> Self {
        // Sixteen random bytes as hex. A page can enumerate paths on a host it
        // can name; it cannot enumerate 128 bits.
        let token: String = (0..16)
            .map(|_| format!("{:02x}", rand::random::<u8>()))
            .collect();
        Self { upstream, blocked: Default::default(), token, port: Default::default(), allow_local: false }
    }

    /// For a check, not a profile: an IP checker the operator runs on their
    /// own network is a target like any other. Nothing but the check is
    /// served by such a relay, and only while it runs (`Via`).
    fn reaching_local(mut self) -> Self {
        self.allow_local = true;
        self
    }

    /// Where the launcher points the first tab. Carries the token, and the
    /// token is what makes the page answer: `http://fury.invalid/` on its own
    /// is forwarded upstream like any other name and fails the way a name that
    /// does not resolve fails through any proxy.
    pub fn start_url(&self) -> String {
        format!("http://{START_HOST}/{}/", self.token)
    }


    /// Refuse connections to these hosts.
    ///
    /// Enforced HERE rather than at DNS, and that is the stronger place. A
    /// DNS-level blocker is bypassed by DNS-over-HTTPS, which is a setting
    /// inside the very browser being filtered. Everything a profile sends goes
    /// through this relay by construction — `--proxy-bypass-list=<-loopback>`,
    /// no exceptions — so a refusal here cannot be routed around.
    ///
    /// See `blocklist.rs` for what it can and cannot match.
    pub fn blocking(mut self, list: std::sync::Arc<crate::blocklist::Blocklist>) -> Self {
        self.blocked = list;
        self
    }

    /// Bind on loopback only and serve until the returned handle is dropped.
    /// Port 0 asks the OS for a free port, which is what the launcher uses.
    /// The probe's address for the start page's link, once there is a port.
    fn probe_href(&self) -> Option<String> {
        self.port.get().map(|p| format!("http://127.0.0.1:{p}/{}/probe/", self.token))
    }

    /// How to name this profile's exit on the start page.
    ///
    /// Host and port only — never the credentials. The page is rendered inside
    /// the profile's own browser, where any site it later visits could read the
    /// document if something went wrong; a proxy password does not belong
    /// anywhere near that.
    fn upstream_label(&self) -> String {
        let (host, port, scheme) = match &self.upstream {
            Upstream::Http { host, port, tls, .. } => (host, port, if *tls { "https" } else { "http" }),
            Upstream::Socks5 { host, port, .. } => (host, port, "socks5"),
            // The peer's address is in the config the operator pasted and is
            // not a secret, but it is not reached through this struct — and
            // naming the exit is a courtesy, not a contract, so "wireguard" is
            // the honest answer rather than an address dug out for display.
            Upstream::WireGuard(..) => return "wireguard".to_string(),
            // Named on the start page in the plainest words available, because
            // this is the one mode where the answer to "where does this come
            // out" is "here", and an operator who forgot they ticked it should
            // learn so from the first tab rather than from a ban.
            Upstream::Direct => return "no proxy — this machine's own address".to_string(),
        };
        format!("{scheme}://{host}:{port}")
    }

    pub async fn serve(self, port: u16) -> anyhow::Result<(u16, tokio::task::JoinHandle<()>)> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await?;
        let bound = listener.local_addr()?.port();
        let _ = self.port.set(bound);
        let this = Arc::new(self);

        let handle = tokio::spawn(async move {
            loop {
                let (client, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        // Not straight back to accept: at EMFILE the pending
                        // connection stays queued, the listener stays ready,
                        // and the loop spins writing this line until a
                        // descriptor frees up.
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
        //
        // And answered ONLY on the tokened path. The first version answered
        // every request for `fury.invalid`, which handed any page in the
        // profile a one-line test: `fetch("http://fury.invalid/", {mode:
        // "no-cors"})` resolves here and throws in every real browser, whose
        // resolver has never heard of the name. With the token, a request
        // without it goes upstream like any other host and fails the way an
        // unresolvable name fails through any proxy — which is what a real
        // Chrome behind a real proxy does.
        if method != "CONNECT" {
            if let Some(route) = self.route(&target, &head) {
                let (status, content_type, body): (&str, &str, std::borrow::Cow<'static, str>) = match route {
                    Route::Start => (
                        "200 OK",
                        "text/html; charset=utf-8",
                        start_page(&self.upstream_label(), self.probe_href().as_deref()).into(),
                    ),
                    Route::Probe => ("200 OK", "text/html; charset=utf-8", PROBE_HTML.into()),
                    Route::ProbeJs => ("200 OK", "application/javascript; charset=utf-8", PROBE_JS.into()),
                    Route::ProbeSw => ("200 OK", "application/javascript; charset=utf-8", PROBE_SW.into()),
                    // The probe page offers to POST its dump to a collector;
                    // there is none here, and the page says so when told.
                    Route::Missing => ("404 Not Found", "text/plain; charset=utf-8", "".into()),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n\
                     Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                client.write_all(response.as_bytes()).await?;
                client.write_all(body.as_bytes()).await?;
                return Ok(());
            }
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

        // Before dialling, and before anything is resolved: the CONNECT
        // carries a NAME, which is the only moment this decision can be made
        // on a name at all.
        //
        // 403 rather than 502: a blocked host is a decision, and reporting it
        // as a gateway failure would send whoever is debugging to look at the
        // proxy.
        if self.blocked.blocks(&host) {
            tracing::debug!(target = %host, "blocked by the profile's list");
            let _ = client
                .write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n")
                .await;
            return Ok(());
        }

        // Nothing on this machine or its network is reachable from a profile.
        //
        // Measured 12.09.2026 from https://example.com inside a profile whose
        // upstream was a proxy on the same machine — which is what a VPN
        // client's local SOCKS port is: `fetch("http://127.0.0.1:35000/…")`
        // resolved, and a closed port threw. That is the agent's own API
        // announcing itself to any site that asks. The relay's tokened routes
        // were answered above; everything else aimed at loopback, link-local or
        // RFC 1918 space fails HERE, on the same path as an unreachable
        // upstream, so an open port and a closed one are indistinguishable.
        //
        // This catches what the browser asked for by address or by one of
        // the names for this machine. A public name that resolves to a local
        // address is caught where names are resolved: `dial`, Direct.
        //
        // Except what the profile's own lists put there in as many words: a
        // `@local` entry (fury_shared::domains, "The local network").
        let local = self.blocked.reached_locally(&host);
        // And one more exception: the browser itself going to a page on this
        // machine, which is how a desktop app or CLI signs in (RFC 8252): it
        // opens the provider's login page, and the provider sends the browser
        // back to `http://localhost:<port>/…?code=…`, where the app listens.
        // Refused, the login never finishes (tester, 18.11.2026: "in a normal
        // browser it works, in Fury it does not"). See `loopback_navigation`
        // for why a page still cannot use it to look at this machine.
        let loopback_nav = !local && method != "CONNECT" && loopback_navigation(&method, &host, &head);
        if !local && !loopback_nav && !self.allow_local && is_local_target(&host) {
            tracing::debug!(target = %host, "local address refused");
            let _ = client
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n")
                .await;
            return Ok(());
        }

        // Kill-switch lives here: a failure to reach upstream produces an error
        // response, never a direct connection. And a local host that cannot be
        // reached locally is refused too, never handed to the proxy.
        let dialled = if local {
            dial_local(&host, port).await
        } else if loopback_nav {
            dial_loopback(&host, port).await
        } else {
            self.dial(&host, port).await
        };
        let mut upstream = match dialled {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(target = %host, local, error = %e, "upstream failed — refusing, not falling back");
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
            let mut cleaned = strip_proxy_headers(&head);
            if loopback_nav {
                // One request, then the connection ends. From here on the
                // client socket is piped to the local server as it is, and
                // Chromium may send its next request over the same proxy
                // connection — one a page made, which the check above never
                // sees. Asking the server to close keeps it to this one.
                cleaned = connection_close(&cleaned);
            }
            upstream.write_all(cleaned.as_bytes()).await?;
            if !leftover.is_empty() {
                upstream.write_all(&leftover).await?;
            }
        }

        tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
        Ok(())
    }

    /// `pub(crate)` for `diagnose`, which wants to fail exactly where a
    /// profile would fail, for the same reason.
    pub(crate) async fn dial(&self, host: &str, port: u16) -> Result<Conn, RelayError> {
        match &self.upstream {
            Upstream::Http {
                host: phost,
                port: pport,
                auth,
                tls: true,
            } => {
                let s = connect_upstream(phost, *pport).await?;
                let mut s = handshake("HTTPS", tls_to_proxy(s, phost)).await?;
                handshake(
                    "HTTP",
                    http_connect(&mut s, host, port, auth.as_ref()),
                )
                .await?;
                Ok(Conn::Tls(Box::new(s)))
            }
            Upstream::Http {
                host: phost,
                port: pport,
                auth,
                tls: false,
            } => {
                let mut s = connect_upstream(phost, *pport).await?;
                handshake(
                    "HTTP",
                    http_connect(&mut s, host, port, auth.as_ref()),
                )
                .await?;
                Ok(Conn::Tcp(s))
            }
            Upstream::Socks5 {
                host: phost,
                port: pport,
                auth,
            } => {
                let mut s = connect_upstream(phost, *pport).await?;
                handshake(
                    "SOCKS5",
                    socks5_connect(&mut s, host, port, auth.as_ref()),
                )
                .await?;
                Ok(Conn::Tcp(s))
            }
            // No handshake: there is no proxy to greet. But this is the one
            // upstream that resolves names HERE, so it is where a name that
            // resolves to this machine or its network has to be caught.
            //
            // Measured 20.10.2026 with the 155 core behind a direct relay:
            // from https://example.com, `fetch("https://192-168-0-108.sslip.io:8443/")`
            // opened a connection to 192.168.0.108 — a public DNS name that
            // answers with a private address — while the same address as a
            // literal was refused above. Resolved once, checked, and dialled
            // at what was checked, so a name cannot answer public for the
            // check and private for the connection.
            //
            // Not done for an HTTP or SOCKS5 upstream, even one listening on
            // this machine (a VPN client's local SOCKS port), whose resolver
            // may well be this machine's too. Asking here would send the name
            // of every site the profile opens to the OS resolver, which is
            // leak 4 at the top of this file, and the answer could differ
            // from the one the proxy acts on anyway. That case stays open;
            // `diagnose` at least flags such a proxy ("proxy_is_local").
            Upstream::Direct => {
                let addrs = resolve_here(host, port).await?;
                if !self.allow_local {
                    if let Some(a) = addrs.iter().find(|a| is_local_ip(a.ip())) {
                        return Err(RelayError::ConnectRejected {
                            target: host.to_string(),
                            reason: format!(
                                "resolves to {}, an address on this machine or its network",
                                a.ip()
                            ),
                        });
                    }
                }
                Ok(Conn::Tcp(connect_resolved(host, port, &addrs).await?))
            }
            Upstream::WireGuard(stack, os) => {
                // Resolved INSIDE the tunnel. Doing it out here would hand this
                // machine's resolver the name of every site the profile is
                // about to open — which is the leak, whatever carries the bytes
                // afterwards.
                let addr = stack
                    .resolve(host)
                    .await
                    .map_err(|e| RelayError::ConnectRejected {
                        target: host.to_string(),
                        reason: e.to_string(),
                    })?;
                let stream = stack
                    .dial(std::net::SocketAddr::new(addr, port), *os)
                    .await
                    .map_err(|e| RelayError::ConnectRejected {
                        target: format!("{host}:{port}"),
                        reason: e.to_string(),
                    })?;
                Ok(Conn::Tunnel(stream))
            }
        }
    }
}

impl Upstream {
    /// The persona this upstream carries, for the one kind where the machine's
    /// TCP is ours to shape. Through a proxy the site's TCP peer is the proxy
    /// server, so there is nothing to say and this changes nothing.
    pub fn for_os(self, os: crate::syn_shape::Os) -> Upstream {
        match self {
            Upstream::WireGuard(shared, _) => Upstream::WireGuard(shared, os),
            other => other,
        }
    }
}

/// An HTTP client whose requests leave the way a profile's do: through a relay
/// of its own over `upstream`, so through `socks5_connect` / `http_connect`.
///
/// For the proxy check and the exit lookups. They used to hand the proxy URL to
/// reqwest, whose SOCKS5 client (hyper-util) is a second implementation and a
/// stricter one: it refuses a login reply whose version byte is not 1 and a
/// CONNECT reply whose reserved byte is not 0, both of which this relay and
/// curl take. A tester's residential SOCKS5 proxy (13.10.2026) passed curl from
/// his machine, US exit, while the check said "SOCKS error: failed parsing
/// server response". A check that disagrees with the browser it is checking
/// for is wrong either way, so now there is one implementation.
///
/// The relay stops when this is dropped.
pub struct Via {
    client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
}

impl Via {
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

impl Drop for Via {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn via(upstream: Upstream, timeout: std::time::Duration) -> anyhow::Result<Via> {
    let (port, task) = Relay::new(upstream).reaching_local().serve(0).await?;
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))?)
        .timeout(timeout)
        .build()?;
    Ok(Via { client, task })
}

/// A loopback proxy address that leaves through `upstream`, for a caller that
/// builds its own client. reqwest speaks HTTP and SOCKS proxies but not
/// WireGuard; the relay speaks all three, so a client pointed here does too.
/// The relay stops when the guard is dropped.
pub async fn loopback(upstream: Upstream) -> anyhow::Result<(String, Loopback)> {
    let (port, task) = Relay::new(upstream).reaching_local().serve(0).await?;
    Ok((format!("http://127.0.0.1:{port}"), Loopback(task)))
}

pub struct Loopback(tokio::task::JoinHandle<()>);

impl Drop for Loopback {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Why a request through [`via`] failed, in the relay's own words: the relay
/// answers a failed upstream with a bare 502, so the cause is asked for again
/// with the same handshake, to the same target.
pub async fn explain(upstream: Upstream, endpoint: &str) -> Option<RelayError> {
    let url = reqwest::Url::parse(endpoint).ok()?;
    let host = url.host_str()?.to_string();
    let port = url.port_or_known_default()?;
    let relay = Relay::new(upstream).reaching_local();
    match tokio::time::timeout(std::time::Duration::from_secs(20), relay.dial(&host, port)).await {
        Ok(Err(e)) => Some(e),
        _ => None,
    }
}

/// Did the far end hang up rather than answer?
///
/// Measured against a real SOCKS5 server sent an HTTP CONNECT: it closes
/// without a word, and the close arrives as a reset on the write rather than as
/// an end-of-file on the read — so both spellings have to count, or the test
/// passes on one machine and not on another.
fn hung_up(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
    )
}

/// Open the socket to the proxy itself, within [`CONNECT_TIMEOUT`].
///
/// A timeout is reported as an `io::Error` of kind `TimedOut` rather than as a
/// variant of its own, because `diagnose::classify_io` already turns that kind
/// into the sentence an operator needs and a second spelling of the same fact
/// would be a second place to keep in step.
async fn connect_upstream(host: &str, port: u16) -> Result<TcpStream, RelayError> {
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(RelayError::UpstreamUnreachable(e)),
        Err(_) => Err(RelayError::UpstreamUnreachable(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "{host}:{port} did not complete a connection within {} s",
                CONNECT_TIMEOUT.as_secs()
            ),
        ))),
    }
}

/// A host a `@local` list names, dialled from this machine.
///
/// Resolved by this machine's resolver, because that is the point: a tailnet
/// name exists only in MagicDNS (100.100.100.100), and an office name only in
/// the office DNS. Resolved ONCE and dialled at what came back, so a name
/// cannot answer private for the check and public for the connection.
///
/// Every address must be private (`is_private_network`, which leaves out
/// loopback and link-local). One public address among them and nothing is
/// dialled: that is a public name, or a machine outside the network it lives
/// in, and either way the connection would leave from the real address.
async fn dial_local(host: &str, port: u16) -> Result<Conn, RelayError> {
    let addrs = resolve_here(host, port).await?;
    if !addrs.iter().all(|a| fury_shared::domains::is_private_network(a.ip())) {
        return Err(RelayError::ConnectRejected {
            target: host.to_string(),
            reason: format!(
                "named in a @local list, but resolves to {addrs:?} here — not a private address, \
                 so it is neither dialled from this machine nor sent to the proxy"
            ),
        });
    }
    Ok(Conn::Tcp(connect_resolved(host, port, &addrs).await?))
}

/// This machine's resolver, within [`CONNECT_TIMEOUT`]. Only for the two
/// paths that dial from here: no proxy, and `@local` hosts.
async fn resolve_here(host: &str, port: u16) -> Result<Vec<SocketAddr>, RelayError> {
    let addrs: Vec<SocketAddr> = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| timed_out(host, port, "resolve"))?
        .map_err(RelayError::UpstreamUnreachable)?
        .collect();
    if addrs.is_empty() {
        return Err(RelayError::UpstreamUnreachable(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{host} resolved to no address"),
        )));
    }
    Ok(addrs)
}

/// Connect to addresses already resolved and checked — never the name again.
async fn connect_resolved(host: &str, port: u16, addrs: &[SocketAddr]) -> Result<TcpStream, RelayError> {
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addrs)).await {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(RelayError::UpstreamUnreachable(e)),
        Err(_) => Err(timed_out(host, port, "connect")),
    }
}

fn timed_out(host: &str, port: u16, what: &str) -> RelayError {
    RelayError::UpstreamUnreachable(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("{host}:{port} did not {what} within {} s", CONNECT_TIMEOUT.as_secs()),
    ))
}

/// Run a proxy handshake within [`HANDSHAKE_TIMEOUT`], and read silence as an
/// answer about what the far end is.
async fn handshake<T>(
    expected: &'static str,
    f: impl std::future::Future<Output = Result<T, RelayError>>,
) -> Result<T, RelayError> {
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, f).await {
        Ok(r) => r,
        Err(_) => Err(RelayError::WrongProtocol {
            expected,
            saw: format!(
                "it took the connection and then said nothing for {} s. A proxy of another \
                 kind behaves exactly like this — it is still waiting for the protocol it \
                 does speak",
                HANDSHAKE_TIMEOUT.as_secs()
            ),
        }),
    }
}

/// What a dial produced.
///
/// Two shapes, because a proxy hands back a socket and a tunnel hands back one
/// end of a duplex. Everything downstream only ever calls
/// `copy_bidirectional`, which takes any stream — so this exists solely to give
/// the two a common name, rather than to add behaviour.
pub enum Conn {
    Tcp(TcpStream),
    /// An `https://` proxy: the CONNECT tunnel inside a TLS session with it.
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
    Tunnel(tokio::io::DuplexStream),
}

impl tokio::io::AsyncRead for Conn {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Tcp(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            Conn::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
            Conn::Tunnel(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for Conn {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Conn::Tcp(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            Conn::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
            Conn::Tunnel(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Tcp(s) => std::pin::Pin::new(s).poll_flush(cx),
            Conn::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
            Conn::Tunnel(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Conn::Tcp(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            Conn::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
            Conn::Tunnel(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

// ---------------------------------------------------------------------------
// Which protocol an address speaks
// ---------------------------------------------------------------------------

/// How long each probe in [`sniff_kind`] may take, connect included.
const SNIFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long to keep waiting for the second probe once the first has said yes.
///
/// Not the full timeout: the common case is an HTTP proxy that answered its
/// CONNECT at once and is sitting on the SOCKS5 greeting waiting for a request
/// line, and that silence is already the answer. Long enough for a port that
/// speaks both to say so from the far side of the world.
const SNIFF_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// `http` or `socks5`, when exactly one of them answers at this address.
///
/// For a pasted `host:port:user:pass`, which names no protocol. The form used
/// to keep whichever type button was pressed, SOCKS5 by default, and an HTTP
/// proxy saved as SOCKS5 looks exactly like a dead one — measured 23.09.2026 on
/// an operator's proxy that curl reached in a second as HTTP and not at all as
/// SOCKS5, and which he had been told was "off".
///
/// Two greetings, side by side, each on its own socket, and neither carries the
/// credentials: whether the far end is SOCKS5 or HTTP is decided by the first
/// bytes it sends back, before any login. The CONNECT names a reserved `.invalid`
/// host, so an open proxy is not asked to reach anybody real.
///
/// `None` when neither answers (dead, filtered, or something else) and when
/// both do (a port that speaks both, which several providers run): in both
/// cases the button the operator pressed is better information than a guess.
pub async fn sniff_kind(host: &str, port: u16) -> Option<&'static str> {
    sniff_kind_within(host, port, SNIFF_TIMEOUT, SNIFF_GRACE).await
}

async fn sniff_kind_within(
    host: &str,
    port: u16,
    limit: std::time::Duration,
    grace: std::time::Duration,
) -> Option<&'static str> {
    // A probe that runs out of time said no.
    let http = tokio::time::timeout(limit, answers_http(host, port));
    let socks = tokio::time::timeout(limit, answers_socks5(host, port));
    tokio::pin!(http, socks);

    let (first_http, first) = tokio::select! {
        h = &mut http => (true, h.unwrap_or(false)),
        s = &mut socks => (false, s.unwrap_or(false)),
    };
    // A yes from the first shortens the wait for the second; a no does not,
    // because then the second is the only evidence there is.
    let wait = if first { grace } else { limit };
    let second = if first_http {
        tokio::time::timeout(wait, &mut socks).await
    } else {
        tokio::time::timeout(wait, &mut http).await
    };
    let second = matches!(second, Ok(Ok(true)));
    let (h, s) = if first_http { (first, second) } else { (second, first) };
    match (h, s) {
        (true, false) => Some("http"),
        (false, true) => Some("socks5"),
        _ => None,
    }
}

/// Does the far end answer a CONNECT with an HTTP status line? Any status: a
/// 407 asking for the password is as much an HTTP proxy as a 200.
async fn answers_http(host: &str, port: u16) -> bool {
    let Ok(mut s) = TcpStream::connect((host, port)).await else { return false };
    let req = b"CONNECT example.invalid:443 HTTP/1.1\r\nHost: example.invalid:443\r\n\r\n";
    if s.write_all(req).await.is_err() {
        return false;
    }
    let mut head = [0u8; 5];
    s.read_exact(&mut head).await.is_ok() && &head == b"HTTP/"
}

/// Does the far end answer a SOCKS5 greeting with a SOCKS5 method choice?
/// Offers no-auth and username/password, as the relay does, and stops there.
async fn answers_socks5(host: &str, port: u16) -> bool {
    let Ok(mut s) = TcpStream::connect((host, port)).await else { return false };
    if s.write_all(&[0x05, 0x02, 0x00, 0x02]).await.is_err() {
        return false;
    }
    let mut resp = [0u8; 2];
    s.read_exact(&mut resp).await.is_ok() && resp[0] == 0x05 && matches!(resp[1], 0x00 | 0x02 | 0xff)
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
    if let Err(e) = s.read_exact(&mut resp).await {
        if hung_up(&e) {
            return Err(RelayError::WrongProtocol {
                expected: "SOCKS5",
                saw: "it closed the connection without answering the greeting. A proxy of \
                      another kind does this — try http://"
                    .to_string(),
            });
        }
        return Err(e.into());
    }
    if resp[0] != 0x05 {
        // This used to be AuthRejected, which is the wrong sentence for it: a
        // version byte that is not 5 says the far end is not a SOCKS5 server,
        // and nothing at all about the password. `H` is the common case —
        // an HTTP proxy replying `HTTP/1.1 400` to a greeting it could not
        // parse — and naming it turns the fix into a dropdown.
        return Err(RelayError::WrongProtocol {
            expected: "SOCKS5",
            saw: if resp[0] == b'H' {
                "it answered in HTTP. This is an HTTP proxy — address it as http://".to_string()
            } else {
                format!("it answered 0x{:02x} where a SOCKS5 version byte belongs", resp[0])
            },
        });
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
        0xff if auth.is_some() => return Err(RelayError::NoLoginAccepted),
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

/// The one sentence for "this is not an HTTP proxy", in the three places that
/// reach that conclusion.
fn wrong_http() -> RelayError {
    RelayError::WrongProtocol {
        expected: "HTTP",
        saw: "it closed the connection without answering. A SOCKS proxy does this with an \
              HTTP request — address it as socks5://"
            .to_string(),
    }
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
// TLS to an https:// proxy
// ---------------------------------------------------------------------------

/// One client config for every https:// proxy in the process: reading the
/// system's trusted roots costs tens of milliseconds, too much per tab.
///
/// The system's roots, as the proxy check's own client uses (reqwest with
/// `rustls-tls-native-roots`), so a proxy the check trusts is one the profile
/// trusts. `ring`, as everywhere else in the tree: aws-lc would bring a C
/// toolchain into the Windows build. ALPN says http/1.1, as curl does, because
/// what follows is a CONNECT and nothing else.
fn proxy_tls_config() -> Arc<tokio_rustls::rustls::ClientConfig> {
    use tokio_rustls::rustls;
    static CONFIG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let found = rustls_native_certs::load_native_certs();
            for e in &found.errors {
                tracing::warn!(error = %e, "a trusted root could not be read");
            }
            let mut roots = rustls::RootCertStore::empty();
            let (added, ignored) = roots.add_parsable_certificates(found.certs);
            tracing::debug!(added, ignored, "trusted roots for https:// proxies");
            let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("ring supports TLS 1.2 and 1.3")
            .with_root_certificates(roots)
            .with_no_client_auth();
            config.alpn_protocols = vec![b"http/1.1".to_vec()];
            Arc::new(config)
        })
        .clone()
}

/// Open the TLS session with an https:// proxy, and name what went wrong in
/// terms of the dropdown when it was not a TLS proxy at all.
async fn tls_to_proxy(
    s: TcpStream,
    host: &str,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, RelayError> {
    tls_to_proxy_with(s, host, proxy_tls_config()).await
}

/// The roots as a parameter, for the tests' own CA.
async fn tls_to_proxy_with(
    s: TcpStream,
    host: &str,
    config: Arc<tokio_rustls::rustls::ClientConfig>,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, RelayError> {
    use tokio_rustls::rustls::pki_types::ServerName;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let name = ServerName::try_from(bare.to_string())
        .map_err(|_| RelayError::ProxyTls(format!("{host} is not a name a certificate can be checked against")))?;
    tokio_rustls::TlsConnector::from(config)
        .connect(name, s)
        .await
        .map_err(|e| tls_failure(&e))
}

fn tls_failure(e: &io::Error) -> RelayError {
    use tokio_rustls::rustls::Error as Tls;
    match e.get_ref().and_then(|inner| inner.downcast_ref::<Tls>()) {
        // The first bytes back were not a TLS record. In practice that is
        // "HTTP/1.1 400 Bad Request": a plain HTTP proxy that read the
        // ClientHello as a request line, the "HTTPS proxy" of many sellers.
        Some(Tls::InvalidMessage(_)) => RelayError::WrongProtocol {
            expected: "HTTPS",
            saw: "it answered in plain text, not TLS. Many proxies sold as \"HTTPS\" are \
                  ordinary HTTP proxies: address it as http://"
                .to_string(),
        },
        Some(Tls::InvalidCertificate(why)) => RelayError::ProxyCertificate(format!("{why:?}")),
        Some(other) => RelayError::ProxyTls(other.to_string()),
        // A SOCKS5 server, and some HTTP proxies, hang up on a ClientHello
        // without a word.
        None if hung_up(e) => RelayError::WrongProtocol {
            expected: "HTTPS",
            saw: "it closed the connection when TLS began. Many proxies sold as \"HTTPS\" are \
                  ordinary HTTP proxies: address it as http:// (or socks5:// for a SOCKS proxy)"
                .to_string(),
        },
        None => RelayError::ProxyTls(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// HTTP CONNECT
// ---------------------------------------------------------------------------

async fn http_connect<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    s: &mut S,
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
    if let Err(e) = s.write_all(req.as_bytes()).await {
        if hung_up(&e) {
            return Err(wrong_http());
        }
        return Err(e.into());
    }

    let (head, _) = match read_request_head(s).await {
        Ok(v) => v,
        Err(RelayError::Io(e)) if hung_up(&e) => return Err(wrong_http()),
        // A SOCKS5 server reads the `C` of CONNECT as a version byte it does
        // not know and hangs up without a word. From here that is an empty
        // answer, and "malformed request" would blame the browser for it.
        Err(RelayError::BadRequest) => return Err(wrong_http()),
        Err(e) => return Err(e),
    };
    if !head.starts_with("HTTP/") {
        return Err(RelayError::WrongProtocol {
            expected: "HTTP",
            saw: "its answer does not begin with a status line".to_string(),
        });
    }
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
async fn read_request_head<S: tokio::io::AsyncRead + Unpin>(s: &mut S) -> Result<(String, Vec<u8>), RelayError> {
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

/// What the relay answers itself, on the tokened path of either of its two
/// names: `fury.invalid` (the start page) and its own loopback address (the
/// probe, which needs a secure context — see `probe_url`).
#[derive(Debug, PartialEq)]
enum Route {
    Start,
    Probe,
    ProbeJs,
    ProbeSw,
    /// Ours by host and token, but no such page.
    Missing,
}

impl Relay {
    fn route(&self, target: &str, head: &str) -> Option<Route> {
        let host = absolute_uri_host(target)
            .or_else(|| header_host(head))
            .unwrap_or_default();
        let ours = is_ours(&host, self.port.get().copied());
        if !ours {
            return None;
        }
        // The path, whichever form the request line took.
        let path = target
            .strip_prefix("http://")
            .and_then(|r| r.find('/').map(|i| &r[i..]))
            .unwrap_or(target);
        let path = path.split('?').next().unwrap_or(path);
        let rest = path.strip_prefix('/')?.strip_prefix(self.token.as_str())?;
        Some(match rest {
            "/" | "" => Route::Start,
            "/probe/" | "/probe" => Route::Probe,
            "/probe/probe.js" => Route::ProbeJs,
            "/probe/sw-probe.js" => Route::ProbeSw,
            _ => Route::Missing,
        })
    }
}

/// Loopback, link-local, RFC 1918 or a name for this machine — anything a
/// page must not be able to reach through the profile.
///
/// Decided on what the browser ASKED for, before anything is resolved: an
/// address, or one of the names that always mean this machine. A name that
/// merely resolves somewhere local is caught later, where it is resolved —
/// `dial` for a profile without a proxy, the only upstream that resolves
/// names on this machine. See the comment there for why the other upstreams
/// are not asked.
pub(crate) fn is_local_target(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let h = h.trim_end_matches('.');
    if h == "localhost" || h.ends_with(".localhost") || h.ends_with(".local") {
        return true;
    }
    h.parse::<std::net::IpAddr>().is_ok_and(is_local_ip)
}

/// Is this the browser opening a page on this machine, rather than a page
/// reaching into it?
///
/// Only plain-HTTP GETs to a loopback host (`localhost`, `*.localhost`,
/// 127.0.0.0/8, ::1) — never the LAN, never CONNECT — and only when the
/// browser says, in `Sec-Fetch-*`, that it is:
///
/// - a top-level navigation (`Sec-Fetch-Mode: navigate` and
///   `Sec-Fetch-Dest: document`): an address typed or pasted, a link, or a
///   provider's redirect at the end of a login. Not an iframe, not fetch(),
///   not an image: those carry another Dest or Mode;
/// - or a request a page on that same local origin makes for itself
///   (`Sec-Fetch-Site: same-origin`), so the page that opened can load its
///   own scripts.
///
/// A page cannot write these headers (they are forbidden request headers),
/// and Chromium sends them to loopback because loopback is a potentially
/// trustworthy origin. So `fetch("http://127.0.0.1:35000/")` from any site
/// is refused exactly as before, and an open port still looks like a closed
/// one to it. What a site CAN do is navigate the whole tab to a local
/// address, as it can in Chrome; it gives up its page to do it and reads
/// nothing back.
fn loopback_navigation(method: &str, host: &str, head: &str) -> bool {
    if !method.eq_ignore_ascii_case("GET") || !is_loopback_host(host) {
        return false;
    }
    let header = |name: &str| {
        head.lines().skip(1).find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_ascii_lowercase())
        })
    };
    let top_level =
        header("sec-fetch-mode").as_deref() == Some("navigate") && header("sec-fetch-dest").as_deref() == Some("document");
    top_level || header("sec-fetch-site").as_deref() == Some("same-origin")
}

/// `localhost`, a name under it, or a loopback address — this machine and
/// nothing else (`is_local_target` also covers the LAN and `.local`).
fn is_loopback_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    let h = h.trim_end_matches('.');
    h == "localhost" || h.ends_with(".localhost") || h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// A loopback host from `loopback_navigation`, dialled here and never sent
/// to the proxy. Names are not resolved: `localhost` and `*.localhost` mean
/// this machine (as Chromium treats them), whatever a hosts file says.
async fn dial_loopback(host: &str, port: u16) -> Result<Conn, RelayError> {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<SocketAddr> = match h.parse::<std::net::IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => vec![
            SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port)),
            SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
        ],
    };
    debug_assert!(addrs.iter().all(|a| a.ip().is_loopback()));
    Ok(Conn::Tcp(connect_resolved(host, port, &addrs).await?))
}

/// The request head with `Connection: close` in place of whatever it had.
fn connection_close(head: &str) -> String {
    let mut out = String::with_capacity(head.len() + 20);
    for (i, line) in head.split("\r\n").enumerate() {
        if line.is_empty() {
            continue;
        }
        if i > 0 && line.to_ascii_lowercase().starts_with("connection:") {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    out
}

/// The address half of `is_local_target`, for addresses a name resolved to.
pub(crate) fn is_local_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || (o[0] == 100 && (64..=127).contains(&o[1])) // carrier-grade NAT
        }
        std::net::IpAddr::V6(v6) => {
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local
                || (seg[0] & 0xffc0) == 0xfe80 // link-local
                || v6.to_ipv4_mapped().is_some_and(|m| is_local_ip(m.into()))
        }
    }
}

/// Is this host one of the relay's own two names?
fn is_ours(host: &str, port: Option<u16>) -> bool {
    let (name, p) = match host.rsplit_once(':') {
        Some((n, p)) => (n, p.parse::<u16>().ok()),
        None => (host, None),
    };
    if name == START_HOST {
        return true;
    }
    matches!((name, p, port), ("127.0.0.1" | "localhost", Some(a), Some(b)) if a == b)
}

/// The detect-suite probe, carried in the binary so a profile can be asked
/// "what does a site see" without a collector, a checkout or a network. The
/// same files CI captures baselines with: tools/detect-suite is the one place
/// the probe is written.
pub(crate) const PROBE_HTML: &str = include_str!("../../tools/detect-suite/probe.html");
pub(crate) const PROBE_JS: &str = include_str!("../../tools/detect-suite/probe.js");
pub(crate) const PROBE_SW: &str = include_str!("../../tools/detect-suite/sw-probe.js");

/// The hostname the start page answers on.
///
/// A `.invalid` name by design (RFC 6761): it can never resolve in DNS, so if
/// this interception ever failed to fire, the request would fail loudly instead
/// of quietly reaching a real site of that name.
pub const START_HOST: &str = "fury.invalid";

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
fn start_page(exit: &str, probe: Option<&str>) -> String {
    // Escaped here rather than by the caller. The function that interpolates is
    // the one that has to be safe: a caller that forgets is a bug nobody sees
    // until a proxy hostname contains a bracket.
    let exit = escape_html(exit);
    // The full detect-suite probe, one click away. The page above measures ten
    // things; the probe measures every vector the suite knows, across workers
    // and iframes, in a secure context — see `Route::Probe`.
    let probe = probe
        .map(|href| format!(r#"<p class="sub" style="margin-top:8px"><a href="{}" style="color:#e0552f">Full probe — every vector, every context →</a></p>"#, escape_html(href)))
        .unwrap_or_default();
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
 {probe}
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

    /// A port that answers whichever of the two greetings it is told to and
    /// behaves like the real thing to the other: an HTTP proxy sits on a SOCKS5
    /// greeting waiting for the rest of a request line, a SOCKS5 server hangs
    /// up on a byte that is not 5.
    async fn speaking(http: bool, socks: bool) -> u16 {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut first = [0u8; 1];
                    if s.read_exact(&mut first).await.is_err() {
                        return;
                    }
                    let mut rest = [0u8; 256];
                    match (first[0], http, socks) {
                        (0x05, _, true) => {
                            let _ = s.read_exact(&mut rest[..3]).await;
                            let _ = s.write_all(&[0x05, 0x02]).await;
                        }
                        (b'C', true, _) => {
                            let _ = s.read(&mut rest).await;
                            let _ = s.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").await;
                        }
                        (0x05, true, false) => {
                            // Still waiting for "\r\n\r\n".
                            let _ = s.read(&mut rest).await;
                            std::future::pending::<()>().await;
                        }
                        _ => {}
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                });
            }
        });
        port
    }

    async fn sniff(port: u16) -> Option<&'static str> {
        use std::time::Duration;
        sniff_kind_within("127.0.0.1", port, Duration::from_secs(2), Duration::from_millis(200)).await
    }

    #[tokio::test]
    async fn an_http_proxy_is_named_http() {
        assert_eq!(sniff(speaking(true, false).await).await, Some("http"));
    }

    #[tokio::test]
    async fn a_socks5_server_is_named_socks5() {
        assert_eq!(sniff(speaking(false, true).await).await, Some("socks5"));
    }

    /// Several providers run both on one port. Either would work, so the
    /// operator's own choice stands.
    #[tokio::test]
    async fn a_port_that_speaks_both_is_left_to_the_operator() {
        assert_eq!(sniff(speaking(true, true).await).await, None);
    }

    #[tokio::test]
    async fn a_port_that_speaks_neither_is_left_to_the_operator() {
        assert_eq!(sniff(speaking(false, false).await).await, None);
        let closed = {
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            l.local_addr().unwrap().port()
        };
        assert_eq!(sniff(closed).await, None);
    }

    // ---- https:// proxies (issue 37) -------------------------------------

    const TEST_CA: &[u8] = include_bytes!("../testdata/proxy-tls/ca.pem");
    const TEST_PROXY_CERT: &[u8] = include_bytes!("../testdata/proxy-tls/proxy.pem");
    const TEST_PROXY_KEY: &[u8] = include_bytes!("../testdata/proxy-tls/proxy.key");

    fn ring() -> Arc<tokio_rustls::rustls::crypto::CryptoProvider> {
        Arc::new(tokio_rustls::rustls::crypto::ring::default_provider())
    }

    /// A client config that trusts the test CA and nothing else.
    fn trusting_test_ca() -> Arc<tokio_rustls::rustls::ClientConfig> {
        use tokio_rustls::rustls::pki_types::{pem::PemObject, CertificateDer};
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        roots.add(CertificateDer::from_pem_slice(TEST_CA).unwrap()).unwrap();
        let mut c = tokio_rustls::rustls::ClientConfig::builder_with_provider(ring())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        c.alpn_protocols = vec![b"http/1.1".to_vec()];
        Arc::new(c)
    }

    /// A real HTTPS proxy on loopback: TLS with a certificate for localhost
    /// and 127.0.0.1 from the test CA, then CONNECT inside the session. It
    /// answers 407 without the right login, 200 with it, and then echoes, so
    /// a test can see bytes cross the tunnel. Returns the port and the
    /// request heads it received.
    async fn tls_proxy() -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio_rustls::rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
        let config = tokio_rustls::rustls::ServerConfig::builder_with_provider(ring())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from_pem_slice(TEST_PROXY_CERT).unwrap()],
                PrivateKeyDer::from_pem_slice(TEST_PROXY_KEY).unwrap(),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            while let Ok((s, _)) = l.accept().await {
                let acceptor = acceptor.clone();
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    let Ok(mut s) = acceptor.accept(s).await else { return };
                    let Ok((head, _)) = read_request_head(&mut s).await else { return };
                    log.lock().unwrap().push(head.clone());
                    // "u:p"
                    if !head.contains("Proxy-Authorization: Basic dTpw") {
                        let _ = s.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").await;
                        return;
                    }
                    let _ = s.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
                    let mut buf = [0u8; 64];
                    while let Ok(n) = s.read(&mut buf).await {
                        if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        (port, seen)
    }

    fn login() -> Option<Credentials> {
        Some(Credentials { username: "u".into(), password: "p".into() })
    }

    #[test]
    fn https_and_http_addresses_parse_to_tls_and_plain() {
        let tls = |u: &str| match crate::parse_upstream(u).unwrap() {
            Upstream::Http { tls, .. } => tls,
            _ => panic!("{u}: wrong variant"),
        };
        assert!(tls("https://u:p@proxy.example:8443"));
        assert!(!tls("http://u:p@proxy.example:8080"));
    }

    /// The issue itself: CONNECT goes inside TLS, the login with it, and the
    /// tunnel carries bytes both ways.
    #[tokio::test]
    async fn an_https_proxy_gets_its_connect_inside_tls() {
        let (port, seen) = tls_proxy().await;
        for host in ["localhost", "127.0.0.1"] {
            let s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let mut s = tls_to_proxy_with(s, host, trusting_test_ca()).await.unwrap();
            http_connect(&mut s, "example.com", 443, login().as_ref()).await.unwrap();
            s.write_all(b"ping").await.unwrap();
            let mut back = [0u8; 4];
            s.read_exact(&mut back).await.unwrap();
            assert_eq!(&back, b"ping", "through {host}");
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].starts_with("CONNECT example.com:443 HTTP/1.1"), "{}", seen[0]);
    }

    #[tokio::test]
    async fn a_wrong_login_inside_tls_is_still_a_wrong_login() {
        let (port, _) = tls_proxy().await;
        let s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let mut s = tls_to_proxy_with(s, "localhost", trusting_test_ca()).await.unwrap();
        let bad = Credentials { username: "u".into(), password: "wrong".into() };
        assert!(matches!(
            http_connect(&mut s, "example.com", 443, Some(&bad)).await,
            Err(RelayError::AuthRejected)
        ));
    }

    /// Through `dial`, as a profile reaches it, with the system's roots: the
    /// test CA is not among them, so this is the certificate refusal, not a
    /// silent pass. A proxy whose certificate is not checked hands its
    /// password to whoever sits in between.
    #[tokio::test]
    async fn an_untrusted_certificate_is_refused_not_ignored() {
        let (port, seen) = tls_proxy().await;
        let relay = Relay::new(Upstream::Http { host: "127.0.0.1".into(), port, auth: login(), tls: true });
        match relay.dial("example.com", 443).await.map(|_| ()) {
            Err(RelayError::ProxyCertificate(why)) => assert!(why.contains("UnknownIssuer"), "{why}"),
            other => panic!("expected a certificate refusal, got {other:?}"),
        }
        assert!(seen.lock().unwrap().is_empty(), "the CONNECT left before the certificate was trusted");
    }

    /// The "HTTPS proxy" of many sellers: plain HTTP with CONNECT. It reads
    /// the ClientHello as a request line and answers 400 in plain text.
    #[tokio::test]
    async fn a_plain_http_proxy_saved_as_https_is_told_to_use_http() {
        let (host, port) = fake_proxy(b"HTTP/1.1 400 Bad Request\r\n\r\n", false).await;
        let relay = Relay::new(Upstream::Http { host, port, auth: None, tls: true });
        match relay.dial("example.com", 443).await.map(|_| ()) {
            Err(RelayError::WrongProtocol { expected, saw }) => {
                assert_eq!(expected, "HTTPS");
                assert!(saw.contains("http://"), "{saw}");
            }
            other => panic!("expected a protocol mismatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_proxy_that_hangs_up_on_tls_is_named_too() {
        let (host, port) = fake_proxy(b"", true).await;
        let relay = Relay::new(Upstream::Http { host, port, auth: None, tls: true });
        match relay.dial("example.com", 443).await.map(|_| ()) {
            Err(RelayError::WrongProtocol { expected, saw }) => {
                assert_eq!(expected, "HTTPS");
                assert!(saw.contains("http://"), "{saw}");
            }
            other => panic!("expected a protocol mismatch, got {other:?}"),
        }
    }

    #[test]
    fn the_start_page_names_an_https_proxy_as_https() {
        let r = Relay::new(Upstream::Http { host: "h".into(), port: 1, auth: None, tls: true });
        assert_eq!(r.upstream_label(), "https://h:1");
    }

    /// A listener that accepts one connection and then does what `then` says.
    ///
    /// Real sockets rather than a trait: the failures under test are things a
    /// far end does on the wire — answers in another protocol, hangs up, says
    /// nothing — and a mock that implements the happy path would not be able to
    /// do any of them.
    async fn fake_proxy(then: &'static [u8], hang_up: bool) -> (String, u16) {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut s, _)) = l.accept().await {
                use tokio::io::AsyncWriteExt;
                if !then.is_empty() {
                    let _ = s.write_all(then).await;
                }
                if hang_up {
                    let _ = s.shutdown().await;
                } else {
                    // Hold the socket open and say nothing further. Dropping it
                    // would be an answer of its own.
                    std::future::pending::<()>().await;
                }
            }
        });
        ("127.0.0.1".to_string(), port)
    }

    /// The failure this whole change exists for, measured 22.09.2026: a mobile
    /// HTTP proxy saved as socks5.
    ///
    /// The old code read the `H` of `HTTP/1.1` as a SOCKS version byte, decided
    /// it was not 5, and returned AuthRejected — so the interface told the
    /// operator their password was refused, and they went to their provider to
    /// reset a password that had never been wrong.
    #[tokio::test]
    async fn an_http_proxy_addressed_as_socks5_names_the_protocol_not_the_password() {
        let (host, port) = fake_proxy(b"HTTP/1.1 400 Bad Request\r\n\r\n", false).await;
        let relay = Relay::new(Upstream::Socks5 { host, port, auth: None });

        // Mapped to () so a failure can be printed: a live socket is not Debug.
        match relay.dial("example.com", 443).await.map(|_| ()) {
            Err(RelayError::WrongProtocol { expected, saw }) => {
                assert_eq!(expected, "SOCKS5");
                assert!(saw.contains("http://"), "{saw}");
            }
            other => panic!("expected a protocol mismatch, got {other:?}"),
        }
    }

    /// The other direction: a SOCKS5 server reads the `C` of CONNECT as a
    /// version it does not know and hangs up without a word.
    #[tokio::test]
    async fn a_socks_proxy_addressed_as_http_names_the_protocol() {
        let (host, port) = fake_proxy(b"", true).await;
        let relay = Relay::new(Upstream::Http { host, port, auth: None, tls: false });

        // Mapped to () so a failure can be printed: a live socket is not Debug.
        match relay.dial("example.com", 443).await.map(|_| ()) {
            Err(RelayError::WrongProtocol { expected, saw }) => {
                assert_eq!(expected, "HTTP");
                assert!(saw.contains("socks5://"), "{saw}");
            }
            other => panic!("expected a protocol mismatch, got {other:?}"),
        }
    }

    /// Silence has to end somewhere.
    ///
    /// There was no bound on the handshake at all until now, so an upstream
    /// that took the connection and said nothing left every tab in the profile
    /// loading forever — no error page, nothing in the log, nothing to act on.
    /// `diagnose` had its own timeout, which is why the step-by-step check
    /// reported a fault the launch path could not.
    ///
    /// Paused time: the read never completes, so the runtime goes idle and the
    /// clock jumps to the timeout instead of the test taking eight seconds.
    #[tokio::test(start_paused = true)]
    async fn a_proxy_that_takes_the_connection_and_says_nothing_gives_up() {
        let (host, port) = fake_proxy(b"", false).await;
        // Connected here rather than through `dial`, so the only timer in this
        // test is the handshake's. Under a paused clock a second timeout could
        // fire first and the test would pass for the wrong reason.
        let mut s = TcpStream::connect((host.as_str(), port)).await.unwrap();

        match handshake("SOCKS5", socks5_connect(&mut s, "example.com", 443, None)).await {
            Err(RelayError::WrongProtocol { expected, saw }) => {
                assert_eq!(expected, "SOCKS5");
                assert!(saw.contains("said nothing"), "{saw}");
            }
            other => panic!("expected the handshake to give up, got {other:?}"),
        }
    }

    /// With no proxy there is no handshake to get wrong: the relay opens the
    /// socket itself. The listener here answers nothing — reaching it is the
    /// whole assertion.
    #[tokio::test]
    async fn with_no_proxy_the_relay_dials_out_itself() {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = l.accept().await;
            std::future::pending::<()>().await;
        });

        // A relay for a check may reach this machine (`reaching_local`); a
        // profile's may not, which is the next test.
        let relay = Relay::new(Upstream::Direct).reaching_local();
        assert!(relay.dial("127.0.0.1", port).await.map(|_| ()).is_ok());
    }

    /// A name that resolves to this machine, through a relay with no proxy.
    ///
    /// `127.1` is not an address to Rust, so `is_local_target` lets it pass as
    /// a name — and the OS resolver reads it as 127.0.0.1 (inet_aton). The
    /// same path a public name with a private answer takes (sslip.io, DNS
    /// rebinding), without needing the network to test it.
    #[tokio::test]
    async fn with_no_proxy_a_name_that_resolves_here_is_refused() {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        let reached = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r = reached.clone();
        tokio::spawn(async move {
            while let Ok((s, _)) = l.accept().await {
                r.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::mem::forget(s);
            }
        });
        assert!(!is_local_target("127.1"), "the premise: the name rule does not see it");

        let relay = Relay::new(Upstream::Direct);
        match relay.dial("127.1", port).await.map(|_| ()) {
            Err(RelayError::ConnectRejected { reason, .. }) => assert!(reason.contains("127.0.0.1"), "{reason}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        // Through the whole relay too: 502, and the listener never hears of it.
        let (rport, task) = relay.serve(0).await.unwrap();
        let answer = connect_through(rport, &format!("127.1:{port}")).await;
        assert!(answer.starts_with("HTTP/1.1 502"), "{answer:?}");
        assert_eq!(reached.load(std::sync::atomic::Ordering::SeqCst), 0);
        task.abort();
    }

    /// The case that was measured: a public DNS name whose answer is a
    /// private address of this machine. Needs DNS and a private address; says
    /// SKIPPED rather than passing quietly without them.
    #[tokio::test]
    async fn with_no_proxy_a_public_name_with_a_private_answer_is_refused() {
        let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let ip = match probe.connect("10.255.255.254:9").and_then(|_| probe.local_addr()) {
            Ok(a) if fury_shared::domains::is_private_network(a.ip()) => a.ip(),
            other => {
                eprintln!("SKIPPED: no private address on this machine ({other:?})");
                return;
            }
        };
        let name = format!("{}.sslip.io", ip.to_string().replace('.', "-"));
        if tokio::net::lookup_host((name.as_str(), 80)).await.map(|mut a| a.any(|a| a.ip() == ip)).unwrap_or(false) == false {
            eprintln!("SKIPPED: {name} does not resolve to {ip} here");
            return;
        }
        let target = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
        let tport = target.local_addr().unwrap().port();
        let reached = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r = reached.clone();
        tokio::spawn(async move {
            while let Ok((s, _)) = target.accept().await {
                r.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::mem::forget(s);
            }
        });

        let (port, task) = Relay::new(Upstream::Direct).serve(0).await.unwrap();
        let answer = connect_through(port, &format!("{name}:{tport}")).await;
        assert!(answer.starts_with("HTTP/1.1 502"), "{answer:?}");
        assert_eq!(reached.load(std::sync::atomic::Ordering::SeqCst), 0, "{name} reached the LAN");
        task.abort();

        // The control: named in a @local list, the same name does open.
        let list = std::sync::Arc::new(crate::blocklist::Blocklist::parse(&format!("@local\n{name}\n")));
        let (port, task) = Relay::new(Upstream::Direct).blocking(list).serve(0).await.unwrap();
        let answer = connect_through(port, &format!("{name}:{tport}")).await;
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer:?}");
        assert_eq!(settled(&reached, 1).await, 1);
        task.abort();
    }

    /// And a public name with a public answer still dials, from the address
    /// it resolved to. Needs DNS; skipped without it.
    #[tokio::test]
    async fn with_no_proxy_a_public_name_still_dials() {
        if tokio::net::lookup_host(("example.com", 80)).await.is_err() {
            eprintln!("SKIPPED: no DNS");
            return;
        }
        let relay = Relay::new(Upstream::Direct);
        assert!(relay.dial("example.com", 80).await.map(|_| ()).is_ok());
    }

    /// A real SOCKS5 refusal still reads as one. Without this the change above
    /// could have turned every authentication failure into "wrong protocol",
    /// which is the same mistake pointing the other way.
    #[tokio::test]
    async fn a_socks5_proxy_that_refuses_the_password_still_says_so() {
        // 0x05 0xFF: version 5, "no acceptable methods".
        let (host, port) = fake_proxy(b"\x05\xff", false).await;
        let relay = Relay::new(Upstream::Socks5 {
            host,
            port,
            auth: Some(Credentials { username: "u".into(), password: "p".into() }),
        });

        // Mapped to () so a failure can be printed: a live socket is not Debug.
        match relay.dial("example.com", 443).await.map(|_| ()) {
            Err(RelayError::NoLoginAccepted) => {}
            other => panic!("expected an auth refusal, got {other:?}"),
        }
    }

    /// A SOCKS5 server that wants a login and bends RFC 1929/1928 in one place,
    /// then answers one HTTP request with a small JSON body, as an IP checker.
    async fn quirky_socks5(auth_reply: &'static [u8], connect_reply: &'static [u8]) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = s.read(&mut buf).await; // greeting
                    let _ = s.write_all(&[0x05, 0x02]).await;
                    let _ = s.read(&mut buf).await; // login
                    let _ = s.write_all(auth_reply).await;
                    let _ = s.read(&mut buf).await; // CONNECT
                    let _ = s.write_all(connect_reply).await;
                    let mut head = Vec::new();
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let body = br#"{"ip":"192.0.2.7"}"#;
                    let _ = s
                        .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes())
                        .await;
                    let _ = s.write_all(body).await;
                });
            }
        });
        port
    }

    /// The check goes the profile's way, so a proxy the browser can use passes
    /// it (tester, 13.10.2026: curl through his SOCKS5 proxy reached ipinfo.io
    /// from his machine; the check said "SOCKS error: failed parsing server
    /// response"). Two departures from the RFCs that reqwest's SOCKS5 client
    /// refuses and this relay, like curl, takes: a login reply with version 5
    /// instead of 1, and a CONNECT reply whose reserved byte is not 0. Which
    /// one his provider sends is not known -- its login does not work from
    /// here -- so both are pinned.
    #[tokio::test]
    async fn a_check_passes_a_proxy_the_profile_can_use() {
        const OK_CONNECT: &[u8] = &[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        const RSV_CONNECT: &[u8] = &[0x05, 0x00, 0x01, 0x01, 0, 0, 0, 0, 0, 0];
        for (name, auth, connect) in [
            ("login reply version 5", &[0x05u8, 0x00][..], OK_CONNECT),
            ("reserved byte 1", &[0x01u8, 0x00][..], RSV_CONNECT),
        ] {
            let port = quirky_socks5(auth, connect).await;
            let upstream = Upstream::Socks5 {
                host: "127.0.0.1".into(),
                port,
                auth: Some(Credentials { username: "u__cr.us;sessttl.60".into(), password: "p".into() }),
            };
            let via = via(upstream, std::time::Duration::from_secs(10)).await.unwrap();
            let body: serde_json::Value = via
                .client()
                .get("http://exit.test/json")
                .send()
                .await
                .unwrap_or_else(|e| panic!("{name}: {e:?}"))
                .json()
                .await
                .unwrap();
            assert_eq!(body["ip"], "192.0.2.7", "{name}");

            // The control: the same server through reqwest's own SOCKS5
            // client gives the tester's sentence.
            let direct = reqwest::Client::builder()
                .proxy(reqwest::Proxy::all(format!("socks5h://u:p@127.0.0.1:{port}")).unwrap())
                .build()
                .unwrap();
            let e = direct.get("http://exit.test/json").send().await.expect_err(name);
            let mut chain = e.to_string();
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                chain.push_str(&format!(" | {s}"));
                src = s.source();
            }
            assert!(chain.contains("failed parsing server response"), "{name}: {chain}");
        }
    }

    /// The blocklist, through the relay rather than through the matcher.
    ///
    /// The matcher has its own tests; this one asserts that the refusal happens
    /// where it is supposed to — BEFORE the upstream is dialled. The upstream
    /// here is a port with nothing on it, so if the block did not fire first
    /// the answer would be 502 rather than 403, and the test would say so.
    #[tokio::test]
    async fn a_blocked_host_is_refused_before_the_upstream_is_dialled() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let list = std::sync::Arc::new(crate::blocklist::Blocklist::parse("doubleclick.net"));
        let relay = Relay::new(Upstream::Http {
            // Nothing listens here. Reaching it at all is the failure.
            host: "127.0.0.1".into(),
            port: 1,
            auth: None,
            tls: false,
        })
        .blocking(list);

        let (port, task) = relay.serve(0).await.unwrap();

        async fn ask(port: u16, host: &str) -> String {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            s.write_all(format!("CONNECT {host}:443 HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut buf = [0u8; 64];
            let n = s.read(&mut buf).await.unwrap_or(0);
            String::from_utf8_lossy(&buf[..n]).to_string()
        }

        let blocked = ask(port, "ad.doubleclick.net").await;
        assert!(blocked.starts_with("HTTP/1.1 403"), "{blocked:?}");

        // The control: an unblocked host reaches the dial and fails there, so
        // the 403 above is the list and not the relay refusing everything.
        let allowed = ask(port, "example.com").await;
        assert!(allowed.starts_with("HTTP/1.1 502"), "{allowed:?}");

        task.abort();
    }


    /// A CONNECT through a relay on `port`, and the first bytes of the answer.
    async fn connect_through(port: u16, target: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = [0u8; 64];
        let n = s.read(&mut buf).await.unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).to_string()
    }

    /// The listener's count, once its accept task has had the chance to run:
    /// the relay answers 200 as soon as connect() returns, which can be before
    /// the accepting side has counted anything.
    async fn settled(counter: &std::sync::atomic::AtomicUsize, want: usize) -> usize {
        for _ in 0..100 {
            let n = counter.load(std::sync::atomic::Ordering::SeqCst);
            if n >= want {
                return n;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        counter.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// An upstream that counts the connections it is offered.
    async fn counting_upstream() -> (Upstream, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = hits.clone();
        tokio::spawn(async move {
            while let Ok((s, _)) = l.accept().await {
                h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                drop(s);
            }
        });
        (Upstream::Http { host: "127.0.0.1".into(), port, auth: None, tls: false }, hits)
    }

    /// A `@local` name that resolves outside private space — here to
    /// loopback, where the agent's own API lives — is refused, and the proxy
    /// never hears of it either.
    #[tokio::test]
    async fn a_local_name_that_resolves_outside_private_space_goes_nowhere() {
        let (upstream, hits) = counting_upstream().await;
        let list = std::sync::Arc::new(crate::blocklist::Blocklist::parse("@local\nlocalhost.localdomain\nfoo.localhost\n"));
        let relay = Relay::new(upstream).blocking(list);
        let (port, task) = relay.serve(0).await.unwrap();

        let answer = connect_through(port, "foo.localhost:443").await;
        assert!(answer.starts_with("HTTP/1.1 502"), "{answer:?}");
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "the name reached the proxy");

        // The control: an ordinary host does go to the proxy.
        let _ = connect_through(port, "example.com:443").await;
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        task.abort();
    }

    /// The whole feature, end to end: a private address of this machine,
    /// named in a `@local` list, is dialled directly and the proxy is not
    /// asked. Unlisted, the same address is refused as before.
    ///
    /// Needs this machine to have a private address on some interface, which
    /// a laptop on any home or office network does. Without one there is
    /// nothing to dial, and the test says so instead of passing quietly.
    #[tokio::test]
    async fn a_listed_private_address_is_dialled_from_this_machine() {
        // The source address the OS would use toward a private network. No
        // packet is sent: a UDP connect only picks the route.
        let probe = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        let ip = match probe.connect("10.255.255.254:9").and_then(|_| probe.local_addr()) {
            Ok(a) if fury_shared::domains::is_private_network(a.ip()) => a.ip(),
            other => {
                eprintln!("SKIPPED: no private address on this machine ({other:?})");
                return;
            }
        };
        let target = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
        let tport = target.local_addr().unwrap().port();
        let reached = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r = reached.clone();
        tokio::spawn(async move {
            while let Ok((s, _)) = target.accept().await {
                r.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::mem::forget(s);
            }
        });

        let (upstream, hits) = counting_upstream().await;
        let list = std::sync::Arc::new(crate::blocklist::Blocklist::parse(&format!("@local\n{ip}\n")));
        let (port, task) = Relay::new(upstream.clone()).blocking(list).serve(0).await.unwrap();
        let answer = connect_through(port, &format!("{ip}:{tport}")).await;
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer:?}");
        assert_eq!(settled(&reached, 1).await, 1);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0, "a local host went to the proxy");
        task.abort();

        let (port, task) = Relay::new(upstream).serve(0).await.unwrap();
        let answer = connect_through(port, &format!("{ip}:{tport}")).await;
        assert!(answer.starts_with("HTTP/1.1 502"), "unlisted: {answer:?}");
        assert_eq!(settled(&reached, 1).await, 1);
        task.abort();
    }

    #[test]
    fn nothing_on_this_machine_or_its_network_is_a_target() {
        for h in [
            "127.0.0.1", "127.8.8.8", "localhost", "LOCALHOST", "foo.localhost", "printer.local",
            "10.0.0.5", "192.168.1.1", "172.16.0.1", "172.31.255.255", "169.254.169.254",
            "100.64.0.1", "0.0.0.0", "::1", "fe80::1", "fd00::1", "::ffff:127.0.0.1",
        ] {
            assert!(is_local_target(h), "{h} should be refused");
        }
        for h in ["localhost.", "printer.local.", "[::1]"] {
            assert!(is_local_target(h), "{h} should be refused");
        }
        for h in ["example.com", "8.8.8.8", "172.32.0.1", "2001:db8::1", "fury.invalid", "11.0.0.1"] {
            assert!(!is_local_target(h), "{h} should pass");
        }
    }

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
        let r = Relay::new(Upstream::Http { host: "h".into(), port: 1, auth: None, tls: false });
        let t = r.token.clone();
        // With the token: answered, in both request forms.
        assert_eq!(r.route(&format!("http://fury.invalid/{t}/"), ""), Some(Route::Start));
        assert_eq!(r.route(&format!("/{t}/"), "Host: fury.invalid\r\n"), Some(Route::Start));
        assert_eq!(r.route(&format!("http://fury.invalid:80/{t}/probe/"), ""), Some(Route::Probe));
        assert_eq!(r.route(&format!("http://fury.invalid/{t}/probe/probe.js?x=1"), ""), Some(Route::ProbeJs));
        assert_eq!(r.route(&format!("http://fury.invalid/{t}/nothing"), ""), Some(Route::Missing));
        // Without it: NOT ours. Forwarded upstream and failing there is what a
        // real browser behind a real proxy does with a name that does not
        // resolve, and answering would be the tell.
        assert_eq!(r.route("http://fury.invalid/", ""), None);
        assert_eq!(r.route("http://fury.invalid/save", ""), None);
        assert_eq!(r.route(&format!("http://fury.invalid/{}x/", &t[..31]), ""), None);
        // Other hosts, never.
        assert_eq!(r.route("http://example.com/", ""), None);
        assert_eq!(r.route("http://notfury.invalid/", ""), None);
        // Loopback only once there is a port, and only on that port.
        assert_eq!(r.route(&format!("http://127.0.0.1:4444/{t}/probe/"), ""), None);
        let _ = r.port.set(4444);
        assert_eq!(r.route(&format!("http://127.0.0.1:4444/{t}/probe/"), ""), Some(Route::Probe));
        assert_eq!(r.route(&format!("http://127.0.0.1:4445/{t}/probe/"), ""), None);
    }

    #[test]
    fn the_start_page_escapes_what_it_prints() {
        // The exit label comes from a proxy hostname the operator typed. It is
        // shown, so it must not be able to close the tag it sits in.
        let page = start_page("<script>alert(1)</script>", None);
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

    #[test]
    fn a_navigation_to_this_machine_is_told_from_a_page_reaching_into_it() {
        let h = |extra: &str| format!("GET http://localhost:59957/auth/ HTTP/1.1\r\nHost: localhost:59957\r\n{extra}\r\n");
        let nav = "Sec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: navigate\r\nSec-Fetch-Dest: document\r\n";
        // The login redirect, a pasted address, and the page loading itself.
        assert!(loopback_navigation("GET", "localhost", &h(nav)));
        assert!(loopback_navigation("GET", "127.0.0.1", &h("sec-fetch-site: none\r\nsec-fetch-mode: navigate\r\nsec-fetch-dest: document\r\n")));
        assert!(loopback_navigation("GET", "[::1]", &h(nav)));
        assert!(loopback_navigation("GET", "app.localhost", &h(nav)));
        assert!(loopback_navigation("GET", "localhost", &h("Sec-Fetch-Site: same-origin\r\nSec-Fetch-Mode: cors\r\nSec-Fetch-Dest: empty\r\n")));
        // What a page could do with it: fetch, an iframe, an image, a form post.
        assert!(!loopback_navigation("GET", "localhost", &h("Sec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: cors\r\nSec-Fetch-Dest: empty\r\n")));
        assert!(!loopback_navigation("GET", "localhost", &h("Sec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: navigate\r\nSec-Fetch-Dest: iframe\r\n")));
        assert!(!loopback_navigation("GET", "localhost", &h("Sec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: no-cors\r\nSec-Fetch-Dest: image\r\n")));
        assert!(!loopback_navigation("POST", "localhost", &h(nav)));
        // No headers at all (not Chromium, or an old one): refused.
        assert!(!loopback_navigation("GET", "localhost", &h("")));
        // Not this machine: the LAN and .local stay closed even to navigation.
        assert!(!loopback_navigation("GET", "192.168.1.10", &h(nav)));
        assert!(!loopback_navigation("GET", "printer.local", &h(nav)));
        assert!(!loopback_navigation("GET", "localhost.evil.com", &h(nav)));
    }

    #[test]
    fn connection_close_replaces_keep_alive() {
        let out = connection_close("GET /a HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n");
        assert_eq!(out, "GET /a HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    }

    /// The tester's case end to end: a login that ends on
    /// `http://localhost:<port>/auth/?code=…`. The navigation reaches the
    /// app on this machine directly (the proxy here is dead, so anything sent
    /// to it would fail), and a page's fetch to the same port does not.
    #[tokio::test]
    async fn a_login_redirect_to_localhost_reaches_the_app_and_a_page_fetch_does_not() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let app = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let aport = app.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let s2 = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = app.accept().await {
                let mut buf = vec![0u8; 4096];
                let n = c.read(&mut buf).await.unwrap_or(0);
                s2.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).to_string());
                let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
            }
        });
        let dead = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let dport = dead.local_addr().unwrap().port();
        drop(dead);
        let relay = Relay::new(Upstream::Socks5 { host: "127.0.0.1".into(), port: dport, auth: None });
        let (rport, task) = relay.serve(0).await.unwrap();

        let ask = |extra: &'static str| async move {
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", rport)).await.unwrap();
            let req = format!(
                "GET http://localhost:{aport}/auth/?code=abc HTTP/1.1\r\nHost: localhost:{aport}\r\n\
                 Proxy-Connection: keep-alive\r\nConnection: keep-alive\r\n{extra}\r\n"
            );
            s.write_all(req.as_bytes()).await.unwrap();
            let mut out = String::new();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), s.read_to_string(&mut out)).await;
            out
        };

        let answer = ask("Sec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: navigate\r\nSec-Fetch-Dest: document\r\n").await;
        assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("ok"), "{answer:?}");
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert!(seen[0].starts_with("GET /auth/?code=abc HTTP/1.1\r\n"), "{:?}", seen[0]);
            assert!(seen[0].contains("\r\nConnection: close\r\n"), "{:?}", seen[0]);
            assert!(!seen[0].to_ascii_lowercase().contains("keep-alive"), "{:?}", seen[0]);
        }

        let answer = ask("Sec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: cors\r\nSec-Fetch-Dest: empty\r\n").await;
        assert!(answer.starts_with("HTTP/1.1 502"), "{answer:?}");
        let answer = ask("Sec-Fetch-Site: cross-site\r\nSec-Fetch-Mode: navigate\r\nSec-Fetch-Dest: iframe\r\n").await;
        assert!(answer.starts_with("HTTP/1.1 502"), "{answer:?}");
        assert_eq!(seen.lock().unwrap().len(), 1, "a page's request reached the app");
        task.abort();
    }
}
