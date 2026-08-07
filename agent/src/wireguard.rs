// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Reading a WireGuard configuration, and refusing the ones that would leak.
//!
//! ## What is here and what is not
//!
//! This parses and validates a `.conf`. It does NOT establish a tunnel, and
//! nothing in the agent yet does — `Config` is accepted, stored and shown, and
//! `relay.rs` cannot dial through it. Saying so in the first paragraph rather
//! than at the bottom, because a half-built VPN is the one feature where a
//! reader assuming otherwise gets an operator's real address on a website.
//!
//! The remaining work is not small and not a shortcut away:
//!
//!   - WireGuard is a NETWORK, not a proxy. The relay dials TCP to an upstream;
//!     a tunnel gives you IP packets. Turning one into the other needs a
//!     userspace TCP stack (`smoltcp`) sitting on a userspace WireGuard
//!     (`boringtun`), because a per-process tunnel is not something macOS or
//!     Windows will give an unprivileged application.
//!   - measured 05.08.2026: those two pull 93 crates and `ring`. That is
//!     acceptable HERE — the agent already depends on `ring` through sqlx and
//!     reqwest — but it is why none of this may go anywhere near `platform-rs`,
//!     which stays thin so it can be cross-checked for Windows from a Mac.
//!
//! ## Why the validation is the interesting half
//!
//! A WireGuard config is four fields and a routing decision, and the routing
//! decision is where profiles leak.
//!
//! `AllowedIPs` says which destinations go through the tunnel. A provider that
//! writes `0.0.0.0/0` is sending everything. One that writes `10.0.0.0/8` is
//! sending their internal network and letting everything else go out of the
//! machine's own interface — which for an anti-detect profile means the site
//! sees the operator's real address, and nothing anywhere reports a fault
//! because the config is doing exactly what it says.
//!
//! So a config that does not carry a default route is REFUSED rather than
//! accepted with a warning. This product's whole claim about the network is
//! that there is no bypass list and no exceptions (`--proxy-bypass-list=<-loopback>`);
//! a split tunnel is an exception list written somewhere else.

use anyhow::{bail, Result};

/// A parsed `[Interface]` / `[Peer]` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The private key, base64, 32 bytes decoded.
    pub private_key: String,
    /// Addresses the interface takes inside the tunnel.
    pub addresses: Vec<String>,
    /// Resolvers to use inside it. Empty means the peer's network decides.
    pub dns: Vec<String>,
    pub peer_public_key: String,
    /// Optional pre-shared key, base64.
    pub preshared_key: Option<String>,
    /// `host:port` of the peer.
    pub endpoint: String,
    pub allowed_ips: Vec<String>,
    pub keepalive: Option<u16>,
}

impl Config {
    /// Reads the format `wg-quick` reads, and refuses what would leak.
    pub fn parse(text: &str) -> Result<Config> {
        let mut section = "";
        let mut iface: Vec<(String, String)> = Vec::new();
        let mut peer: Vec<(String, String)> = Vec::new();

        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                section = match name.trim().to_ascii_lowercase().as_str() {
                    "interface" => "interface",
                    "peer" => "peer",
                    other => bail!("unknown section [{other}]"),
                };
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                bail!("{line:?} is not `Key = Value`");
            };
            let entry = (k.trim().to_ascii_lowercase(), v.trim().to_string());
            match section {
                "interface" => iface.push(entry),
                // A second [Peer] is a real WireGuard feature and this does not
                // implement it. Refusing beats silently using the first.
                "peer" if peer.is_empty() || entry.0 != "publickey" => peer.push(entry),
                "peer" => bail!("more than one [Peer] — this supports a single peer"),
                _ => bail!("{line:?} appears before any section"),
            }
        }

        let get = |list: &[(String, String)], key: &str| -> Option<String> {
            list.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        };
        let split = |v: Option<String>| -> Vec<String> {
            v.map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default()
        };

        let private_key = get(&iface, "privatekey")
            .ok_or_else(|| anyhow::anyhow!("[Interface] has no PrivateKey"))?;
        let peer_public_key = get(&peer, "publickey")
            .ok_or_else(|| anyhow::anyhow!("[Peer] has no PublicKey"))?;
        let endpoint =
            get(&peer, "endpoint").ok_or_else(|| anyhow::anyhow!("[Peer] has no Endpoint"))?;

        check_key(&private_key, "PrivateKey")?;
        check_key(&peer_public_key, "PublicKey")?;
        let preshared_key = get(&peer, "presharedkey");
        if let Some(psk) = &preshared_key {
            check_key(psk, "PresharedKey")?;
        }
        if !endpoint.rsplit_once(':').is_some_and(|(h, p)| {
            !h.is_empty() && p.parse::<u16>().is_ok_and(|n| n != 0)
        }) {
            bail!("Endpoint {endpoint:?} is not host:port");
        }

        let allowed_ips = split(get(&peer, "allowedips"));
        if !carries_default_route(&allowed_ips) {
            bail!(
                "AllowedIPs is {allowed_ips:?}, which is a split tunnel: anything \
                 outside it would leave from this machine's own address rather \
                 than through the peer. Use 0.0.0.0/0 (and ::/0 for IPv6)."
            );
        }

        let keepalive = get(&peer, "persistentkeepalive")
            .map(|v| v.parse::<u16>())
            .transpose()
            .map_err(|_| anyhow::anyhow!("PersistentKeepalive is not a number"))?;

        Ok(Config {
            private_key,
            addresses: split(get(&iface, "address")),
            dns: split(get(&iface, "dns")),
            peer_public_key,
            preshared_key,
            endpoint,
            allowed_ips,
            keepalive,
        })
    }
}

/// A WireGuard key is 32 bytes, base64, which is 44 characters ending in `=`.
fn check_key(value: &str, what: &str) -> Result<()> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| anyhow::anyhow!("{what} is not base64"))?;
    if decoded.len() != 32 {
        bail!("{what} decodes to {} bytes, expected 32", decoded.len());
    }
    Ok(())
}

/// Does this AllowedIPs actually route everything?
///
/// `0.0.0.0/0` is the usual spelling. Some providers write the two halves
/// `0.0.0.0/1, 128.0.0.0/1`, which covers the same space and is how they avoid
/// clobbering a default route on a real interface — so it has to count.
fn carries_default_route(entries: &[String]) -> bool {
    let has = |want: &str| entries.iter().any(|e| e == want);
    has("0.0.0.0/0") || (has("0.0.0.0/1") && has("128.0.0.0/1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    const KEY_B: &str = "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=";

    fn conf(allowed: &str) -> String {
        format!(
            "[Interface]\nPrivateKey = {KEY_A}\nAddress = 10.2.0.2/32\nDNS = 1.1.1.1\n\n\
             [Peer]\nPublicKey = {KEY_B}\nEndpoint = vpn.example.com:51820\n\
             AllowedIPs = {allowed}\nPersistentKeepalive = 25\n"
        )
    }

    #[test]
    fn a_provider_config_reads_the_way_wg_quick_reads_it() {
        let c = Config::parse(&conf("0.0.0.0/0, ::/0")).unwrap();
        assert_eq!(c.private_key, KEY_A);
        assert_eq!(c.peer_public_key, KEY_B);
        assert_eq!(c.endpoint, "vpn.example.com:51820");
        assert_eq!(c.addresses, ["10.2.0.2/32"]);
        assert_eq!(c.dns, ["1.1.1.1"]);
        assert_eq!(c.allowed_ips, ["0.0.0.0/0", "::/0"]);
        assert_eq!(c.keepalive, Some(25));
        assert_eq!(c.preshared_key, None);
    }

    /// The check this module exists for. A split tunnel does exactly what it
    /// says and what it says is that some traffic leaves from the operator's
    /// own address — with nothing reporting a fault.
    #[test]
    fn a_split_tunnel_is_refused_rather_than_warned_about() {
        let err = Config::parse(&conf("10.0.0.0/8")).unwrap_err().to_string();
        assert!(err.contains("split tunnel"), "{err}");
        assert!(err.contains("own address"), "{err}");

        // Not a default route either, however wide it looks.
        assert!(Config::parse(&conf("0.0.0.0/1")).is_err());
        assert!(Config::parse(&conf("128.0.0.0/1")).is_err());
    }

    /// The two-halves spelling providers use to avoid clobbering a real
    /// default route. It covers the same space, so it has to count.
    #[test]
    fn the_two_halves_spelling_counts_as_a_default_route() {
        let c = Config::parse(&conf("0.0.0.0/1, 128.0.0.0/1, ::/0")).unwrap();
        assert_eq!(c.allowed_ips.len(), 3);
    }

    #[test]
    fn keys_are_checked_for_being_keys() {
        let bad = conf("0.0.0.0/0").replace(KEY_A, "not base64!!");
        assert!(Config::parse(&bad).unwrap_err().to_string().contains("base64"));

        // Right shape, wrong length — the mistake a truncated copy-paste makes.
        let short = conf("0.0.0.0/0").replace(KEY_A, "QUFB");
        let err = Config::parse(&short).unwrap_err().to_string();
        assert!(err.contains("32"), "{err}");
    }

    #[test]
    fn the_pieces_a_tunnel_cannot_be_built_without_are_required() {
        for missing in ["PrivateKey", "PublicKey", "Endpoint"] {
            let text: String = conf("0.0.0.0/0")
                .lines()
                .filter(|l| !l.starts_with(missing))
                .collect::<Vec<_>>()
                .join("\n");
            let err = Config::parse(&text).unwrap_err().to_string();
            assert!(err.contains(missing), "removing {missing} gave {err:?}");
        }
    }

    #[test]
    fn an_endpoint_without_a_port_is_named_as_such() {
        let text = conf("0.0.0.0/0").replace("vpn.example.com:51820", "vpn.example.com");
        assert!(Config::parse(&text).unwrap_err().to_string().contains("host:port"));
    }

    /// Multiple peers are a real WireGuard feature that this does not
    /// implement. Refusing beats silently using the first one.
    #[test]
    fn a_second_peer_is_refused_rather_than_ignored() {
        let text = format!("{}\n[Peer]\nPublicKey = {KEY_B}\n", conf("0.0.0.0/0"));
        let err = Config::parse(&text).unwrap_err().to_string();
        assert!(err.contains("single peer"), "{err}");
    }

    #[test]
    fn comments_and_blank_lines_are_not_syntax_errors() {
        let text = format!("# exported by the provider\n\n{}\n\n", conf("0.0.0.0/0"));
        assert!(Config::parse(&text).is_ok());
    }
}
