//! Fury agent — the local daemon.
//!
//! Runs on the operator's machine and is the only component that ever holds
//! decrypted secrets. Owns the proxy relays, launches the core, keeps profile
//! locks alive, and serves the local automation API.
//!
//! Only the pieces that stand on their own today are wired up; see
//! docs/09-roadmap.md for what is still missing.

// Wired into the local API in phase 4; built and tested standalone until then.
#[allow(dead_code)]
mod launcher;
mod relay;

use std::time::Duration;

use fury_shared::fingerprint::samples;
use relay::{Credentials, Relay, Upstream};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fury_agent=info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("relay") => cmd_relay(&args[1..]).await,
        Some("check-fingerprint") => cmd_check_fingerprint(&args[1..]),
        _ => {
            eprintln!(
                "fury-agent {}\n\
                 \n\
                 USAGE:\n  \
                   fury-agent relay <upstream-url> [--port N]\n      \
                     Start a profile relay. Upstream may be:\n        \
                       http://user:pass@host:port\n        \
                       socks5://user:pass@host:port\n  \
                   fury-agent check-fingerprint <config.json>\n      \
                     Validate a fingerprint config for internal consistency.\n      \
                     Pass '-' to check the built-in sample.\n",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}

async fn cmd_relay(args: &[String]) -> anyhow::Result<()> {
    let url = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing upstream url"))?;
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);

    let upstream = parse_upstream(url)?;
    tracing::info!(?upstream, "starting relay");

    let (bound, handle) = Relay::new(upstream).serve(port).await?;
    tracing::info!(
        port = bound,
        "relay listening — launch the core with --proxy-server=http://127.0.0.1:{bound}"
    );

    // Kill-switch note: if the upstream dies, sessions fail closed. The relay
    // stays up so the browser keeps failing rather than silently going direct.
    handle.await?;
    Ok(())
}

fn cmd_check_fingerprint(args: &[String]) -> anyhow::Result<()> {
    let path = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing config path (or '-' for the sample)"))?;

    let config = if path == "-" {
        samples::macos_arm64()
    } else {
        serde_json::from_str(&std::fs::read_to_string(path)?)?
    };

    match config.validate() {
        Ok(()) => {
            println!("consistent — no contradictions found");
            Ok(())
        }
        Err(errs) => {
            eprintln!("{} inconsistency(ies) — this profile would stand out:", errs.len());
            for e in &errs {
                eprintln!("  - {e}");
            }
            std::process::exit(1);
        }
    }
}

/// Parse `scheme://[user:pass@]host:port`.
///
/// Deliberately hand-rolled rather than pulled from a URL crate: proxy strings
/// in the wild contain characters real URL parsers reject, and silently
/// mis-parsing one means connecting somewhere unintended.
fn parse_upstream(url: &str) -> anyhow::Result<Upstream> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("expected scheme://, got {url:?}"))?;

    let (auth, hostport) = match rest.rsplit_once('@') {
        Some((a, hp)) => {
            let (u, p) = a
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("credentials must be user:pass"))?;
            (
                Some(Credentials {
                    username: u.to_string(),
                    password: p.to_string(),
                }),
                hp,
            )
        }
        None => (None, rest),
    };

    let (host, port) = hostport
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("expected host:port"))?;
    let port: u16 = port.parse()?;
    let host = host.to_string();

    Ok(match scheme {
        "http" | "https" => Upstream::Http { host, port, auth },
        // socks5 is treated as socks5h unconditionally: local DNS resolution
        // would leak the operator's resolver. See docs/05.
        "socks5" | "socks5h" => Upstream::Socks5 { host, port, auth },
        other => anyhow::bail!("unsupported proxy scheme {other:?}"),
    })
}

#[allow(dead_code)]
const LOCK_HEARTBEAT: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_authenticated_socks5() {
        let u = parse_upstream("socks5://bob:s3cr3t@10.0.0.1:1080").unwrap();
        match u {
            Upstream::Socks5 { host, port, auth } => {
                assert_eq!(host, "10.0.0.1");
                assert_eq!(port, 1080);
                let a = auth.unwrap();
                assert_eq!(a.username, "bob");
                assert_eq!(a.password, "s3cr3t");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn passwords_may_contain_at_signs() {
        // rsplit_once('@') matters: 'p@ss' would otherwise split in the wrong place.
        let u = parse_upstream("http://bob:p@ss@proxy.example:8080").unwrap();
        match u {
            Upstream::Http { host, auth, .. } => {
                assert_eq!(host, "proxy.example");
                assert_eq!(auth.unwrap().password, "p@ss");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn anonymous_proxies_parse() {
        assert!(matches!(
            parse_upstream("socks5://127.0.0.1:9050").unwrap(),
            Upstream::Socks5 { auth: None, .. }
        ));
    }

    #[test]
    fn unknown_schemes_are_refused() {
        assert!(parse_upstream("ftp://x:1").is_err());
        assert!(parse_upstream("no-scheme:1080").is_err());
    }

    #[test]
    fn the_sample_persona_is_self_consistent() {
        samples::macos_arm64().validate().unwrap();
    }
}
