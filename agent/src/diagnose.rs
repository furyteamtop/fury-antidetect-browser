// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Why a proxy does not work, said in a sentence an operator can act on.
//!
//! `proxies.check` answers yes or no. The no is where the time goes: "could not
//! connect to the proxy" covers a typo in the host, a dead provider, a firewall,
//! the wrong port, and a password the provider rotated — each with a different
//! fix, and the operator has to guess which. AdsPower and Kameleo both ship a
//! step-by-step tester for exactly this reason (docs/12, audit of 12.09.2026).
//!
//! The steps are the steps a connection actually takes, in order, and the first
//! one that fails names the cause:
//!
//! 1. **parse** — is the string a proxy at all
//! 2. **reach** — does anything answer on that host and port
//! 3. **tunnel** — does the proxy accept us and open a connection out: this is
//!    where a bad password or an exhausted plan shows up, distinct from step 2
//! 4. **exit** — what the world sees: address, country, city, network
//!
//! Plus one **note** that is never a failure: whether a VPN tunnel is up on this
//! machine. It does not change the route — the profile goes through the relay
//! and the relay through the proxy regardless — but a proxy on 127.0.0.1 is
//! usually the VPN client itself, and then its exit is the VPN's, which is worth
//! saying before somebody spends an hour on why the country is wrong.
//!
//! Everything here reuses the relay's own dial path, so the tunnel step fails
//! exactly where a profile would fail, for the same reason.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::relay::{Relay, RelayError, Upstream};

#[derive(Debug, Serialize)]
pub struct Step {
    /// `parse`, `reach`, `tunnel`, `exit`.
    pub step: &'static str,
    pub ok: bool,
    /// Milliseconds the step took, when it ran.
    pub ms: Option<u64>,
    /// What happened, for an operator. English here; the interface has the
    /// translations keyed on `code`.
    pub detail: String,
    /// A stable key the interface translates. Absent when the step passed.
    pub code: Option<&'static str>,
}

#[derive(Debug, Serialize, Default)]
pub struct Exit {
    pub ip: Option<String>,
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub timezone: Option<String>,
    /// The network the address belongs to — ipinfo's `org`, "AS15169 Google
    /// LLC". A residential address on a datacenter ASN is the cheapest
    /// proxy-detection there is, so it is shown rather than hidden.
    pub org: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub ok: bool,
    pub steps: Vec<Step>,
    pub exit: Exit,
    /// Observations that are not failures.
    pub notes: Vec<Note>,
}

#[derive(Debug, Serialize)]
pub struct Note {
    pub code: &'static str,
    pub detail: String,
}

fn step(name: &'static str, ok: bool, started: Option<Instant>, detail: String, code: Option<&'static str>) -> Step {
    Step { step: name, ok, ms: started.map(|s| s.elapsed().as_millis() as u64), detail, code }
}

/// Runs every step it can and stops at the first that fails.
pub async fn run(url: &str, checker_url: Option<&str>) -> Report {
    let mut steps = Vec::new();
    let mut notes = Vec::new();

    // 1. parse
    let upstream = match crate::parse_upstream(url) {
        Ok(u) => {
            steps.push(step("parse", true, None, describe(&u), None));
            u
        }
        Err(e) => {
            steps.push(step("parse", false, None, e.to_string(), Some("not_a_proxy")));
            return Report { ok: false, steps, exit: Exit::default(), notes };
        }
    };

    if let Some(n) = vpn_note() {
        notes.push(n);
    }
    if let Some((host, _)) = host_port(&upstream) {
        if crate::relay::is_local_target(host) {
            notes.push(Note {
                code: "proxy_is_local",
                detail: format!(
                    "{host} is this machine or its network: the exit below is whatever is \
                     running there — usually a VPN client — not a remote proxy."
                ),
            });
        }
    }

    // 2. reach
    if let Some((host, port)) = host_port(&upstream) {
        let started = Instant::now();
        match tokio::time::timeout(Duration::from_secs(8), tokio::net::TcpStream::connect((host, port))).await {
            Ok(Ok(_)) => steps.push(step("reach", true, Some(started), format!("{host}:{port} answers"), None)),
            Ok(Err(e)) => {
                let (code, detail) = classify_io(&e, host, port);
                steps.push(step("reach", false, Some(started), detail, Some(code)));
                return Report { ok: false, steps, exit: Exit::default(), notes };
            }
            Err(_) => {
                steps.push(step(
                    "reach",
                    false,
                    Some(started),
                    format!("nothing answered on {host}:{port} within 8 s"),
                    Some("timeout"),
                ));
                return Report { ok: false, steps, exit: Exit::default(), notes };
            }
        }
    } else {
        // WireGuard: there is no host to knock on; the handshake is the tunnel
        // step and the stack is already up if parse succeeded.
        steps.push(step("reach", true, None, "WireGuard peer configured".into(), None));
    }

    // 3. tunnel — through the relay's own dial, to the checker's host.
    let endpoint = checker_endpoint(checker_url);
    let target_host = reqwest::Url::parse(&endpoint)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "ipinfo.io".to_string());
    {
        let started = Instant::now();
        let relay = Relay::new(upstream);
        match tokio::time::timeout(Duration::from_secs(20), relay.dial(&target_host, 443)).await {
            Ok(Ok(_)) => steps.push(step("tunnel", true, Some(started), format!("CONNECT {target_host}:443 accepted"), None)),
            Ok(Err(e)) => {
                let (code, detail) = classify_relay(&e, &target_host);
                steps.push(step("tunnel", false, Some(started), detail, Some(code)));
                return Report { ok: false, steps, exit: Exit::default(), notes };
            }
            Err(_) => {
                steps.push(step(
                    "tunnel",
                    false,
                    Some(started),
                    "the proxy accepted the connection and then went silent".into(),
                    Some("timeout"),
                ));
                return Report { ok: false, steps, exit: Exit::default(), notes };
            }
        }
    }

    // 4. exit
    let started = Instant::now();
    let exit = match fetch_exit(url, &endpoint).await {
        Ok(e) => {
            steps.push(step(
                "exit",
                true,
                Some(started),
                format!(
                    "{} · {}{}",
                    e.ip.clone().unwrap_or_else(|| "?".into()),
                    e.country.clone().unwrap_or_else(|| "?".into()),
                    e.city.as_deref().map(|c| format!(" · {c}")).unwrap_or_default()
                ),
                None,
            ));
            e
        }
        Err(e) => {
            steps.push(step("exit", false, Some(started), e.to_string(), Some("checker_failed")));
            return Report { ok: false, steps, exit: Exit::default(), notes };
        }
    };

    if let Some(org) = exit.org.as_deref() {
        if looks_like_datacenter(org) {
            notes.push(Note {
                code: "datacenter_exit",
                detail: format!(
                    "the exit belongs to {org}, which reads as a hosting network rather than an \
                     ISP; sites that score addresses will notice"
                ),
            });
        }
    }

    Report { ok: true, steps, exit, notes }
}

/// Which protocol this address actually speaks, when the one it was given does
/// not.
///
/// A proxy line as providers hand it out — `host:port:user:pass` — says nothing
/// about whether the far end is SOCKS5 or HTTP, so the form has to default to
/// one of them and is wrong about half the time. Being wrong is cheap to
/// recover from and expensive to diagnose: the failure is silence, and silence
/// is what a firewall, a dead provider and an exhausted plan all look like.
///
/// So when a check fails, try the other family before reporting it. One dial,
/// no request, and only on a failure that already cost the operator fifteen
/// seconds.
///
/// `http` and `https` are one family here: both parse to [`Upstream::Http`],
/// and offering to swap one for the other would be offering to change nothing.
pub async fn speaks_instead(url: &str, checker_url: Option<&str>) -> Option<&'static str> {
    let (scheme, rest) = url.split_once("://")?;
    let candidate = match scheme {
        "socks5" | "socks5h" => "http",
        "http" | "https" => "socks5",
        _ => return None,
    };
    let upstream = crate::parse_upstream(&format!("{candidate}://{rest}")).ok()?;
    let endpoint = checker_endpoint(checker_url);
    let target = reqwest::Url::parse(&endpoint)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "ipinfo.io".to_string());

    match tokio::time::timeout(Duration::from_secs(20), Relay::new(upstream).dial(&target, 443)).await {
        Ok(Ok(_)) => Some(candidate),
        // Anything else means the guess is no better than what was tried. Say
        // nothing rather than send somebody round a second wrong dropdown.
        _ => None,
    }
}

fn describe(u: &Upstream) -> String {
    match u {
        Upstream::Http { host, port, auth } => {
            format!("http proxy {host}:{port}{}", if auth.is_some() { ", with credentials" } else { ", no credentials" })
        }
        Upstream::Socks5 { host, port, auth } => {
            format!("socks5 proxy {host}:{port}{}", if auth.is_some() { ", with credentials" } else { ", no credentials" })
        }
        Upstream::WireGuard(_) => "WireGuard tunnel".to_string(),
        Upstream::Direct => "no proxy: this machine's own address".to_string(),
    }
}

fn host_port(u: &Upstream) -> Option<(&str, u16)> {
    match u {
        Upstream::Http { host, port, .. } | Upstream::Socks5 { host, port, .. } => Some((host.as_str(), *port)),
        Upstream::WireGuard(_) | Upstream::Direct => None,
    }
}

fn checker_endpoint(checker_url: Option<&str>) -> String {
    checker_url
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("FURY_IP_CHECK").ok())
        .unwrap_or_else(|| "https://ipinfo.io/json".to_string())
}

fn classify_io(e: &std::io::Error, host: &str, port: u16) -> (&'static str, String) {
    use std::io::ErrorKind::*;
    match e.kind() {
        ConnectionRefused => ("refused", format!("{host}:{port} refused the connection — the host is up but nothing listens on that port")),
        TimedOut => ("timeout", format!("{host}:{port} did not answer — a firewall, or the wrong address")),
        _ if e.to_string().contains("failed to lookup") || e.to_string().contains("nodename") || e.to_string().contains("Name or service") => {
            ("no_such_host", format!("{host} does not resolve — check the spelling"))
        }
        NotConnected | NetworkUnreachable | HostUnreachable => ("unreachable", format!("no route to {host} — is this machine online?")),
        _ => ("unreachable", format!("{host}:{port}: {e}")),
    }
}

fn classify_relay(e: &RelayError, target: &str) -> (&'static str, String) {
    match e {
        RelayError::AuthRejected => ("auth", "the proxy refused the username or password".to_string()),
        RelayError::ConnectRejected { reason, .. } => (
            "connect_rejected",
            format!("the proxy accepted us but would not open {target}:443 — {reason}. Usually an exhausted plan, a blocked destination, or a proxy that only allows certain ports"),
        ),
        RelayError::UpstreamUnreachable(io) => ("unreachable", format!("the proxy stopped answering: {io}")),
        RelayError::WrongProtocol { expected, saw } => (
            "protocol",
            format!("this address does not speak {expected}: {saw}"),
        ),
        RelayError::BadRequest => ("protocol", "the proxy did not speak the protocol its address claims — http:// for a SOCKS proxy or the other way round".to_string()),
        RelayError::Io(io) => ("protocol", format!("the proxy answered something unexpected: {io}")),
    }
}

async fn fetch_exit(url: &str, endpoint: &str) -> anyhow::Result<Exit> {
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(url)?)
        .timeout(Duration::from_secs(15))
        .build()?;
    let body: serde_json::Value = client.get(endpoint).send().await?.json().await?;
    let s = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Ok(Exit {
        ip: s("ip"),
        country: s("country"),
        region: s("region"),
        city: s("city"),
        timezone: s("timezone"),
        org: s("org"),
    })
}

/// Hosting networks announce themselves in their names. Not a detector — a
/// hint — and the list is the handful that come up in practice.
fn looks_like_datacenter(org: &str) -> bool {
    let o = org.to_ascii_lowercase();
    ["hosting", "datacenter", "data center", "cloud", "server", "digitalocean", "hetzner", "ovh", "amazon", "google llc", "microsoft", "linode", "vultr", "contabo", "leaseweb"]
        .iter()
        .any(|k| o.contains(k))
}

/// Is a VPN tunnel up on this machine? Read from the interface list, which is
/// what the traffic actually uses; a VPN app that is installed but off has no
/// interface.
fn vpn_note() -> Option<Note> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/sbin/ifconfig").output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        // utun0-2 exist on every recent macOS for system services; a VPN adds
        // one with an IPv4 address that is not link-local.
        let mut current: Option<&str> = None;
        for line in text.lines() {
            if !line.starts_with('\t') && !line.starts_with(' ') {
                current = line.split(':').next().filter(|n| n.starts_with("utun") || n.starts_with("ppp") || n.starts_with("ipsec"));
            } else if let Some(name) = current {
                if let Some(addr) = line.trim_start().strip_prefix("inet ") {
                    let ip = addr.split_whitespace().next().unwrap_or("");
                    if !ip.starts_with("169.254.") && !ip.is_empty() {
                        return Some(Note {
                            code: "vpn_active",
                            detail: format!("a VPN tunnel is up on this machine ({name}, {ip}). The profile still leaves through its proxy; only a proxy on this machine would inherit the VPN's exit"),
                        });
                    }
                }
            }
        }
        None
    }
    #[cfg(target_os = "windows")]
    {
        // RAS/WireGuard/OpenVPN adapters show up as extra interfaces; without a
        // reliable cheap read, say nothing rather than guess.
        None
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_string_that_is_not_a_proxy_stops_at_parse() {
        let r = run("not a proxy", None).await;
        assert!(!r.ok);
        assert_eq!(r.steps.len(), 1);
        assert_eq!(r.steps[0].code, Some("not_a_proxy"));
    }

    #[tokio::test]
    async fn a_closed_port_is_refused_not_unreachable() {
        // Bind and drop: the port exists, nothing listens.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let r = run(&format!("http://127.0.0.1:{port}"), None).await;
        assert!(!r.ok);
        let reach = r.steps.iter().find(|s| s.step == "reach").expect("reach ran");
        assert_eq!(reach.code, Some("refused"), "{reach:?}");
        assert!(r.notes.iter().any(|n| n.code == "proxy_is_local"), "a loopback proxy is named as local");
        assert!(r.steps.iter().all(|s| s.step != "tunnel"), "stops at the first failure");
    }

    #[tokio::test]
    async fn a_proxy_that_refuses_the_password_says_so() {
        // A fake HTTP proxy answering 407 to everything.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = tokio::io::AsyncReadExt::read(&mut s, &mut buf).await;
                let _ = tokio::io::AsyncWriteExt::write_all(
                    &mut s,
                    b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\nContent-Length: 0\r\n\r\n",
                )
                .await;
            }
        });
        let r = run(&format!("http://user:wrong@127.0.0.1:{port}"), None).await;
        assert!(!r.ok);
        assert!(r.steps.iter().find(|s| s.step == "reach").unwrap().ok);
        let tunnel = r.steps.iter().find(|s| s.step == "tunnel").expect("tunnel ran");
        assert_eq!(tunnel.code, Some("auth"), "{tunnel:?}");
    }

    #[test]
    fn hosting_networks_are_named() {
        assert!(looks_like_datacenter("AS14061 DigitalOcean, LLC"));
        assert!(looks_like_datacenter("AS24940 Hetzner Online GmbH"));
        assert!(!looks_like_datacenter("AS8359 MTS PJSC"));
    }
}
