// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Refusing connections to hosts a profile does not want to talk to.
//!
//! Donut Browser calls its equivalent a per-profile DNS blocker. This is not
//! one, and the difference is worth stating because it is in our favour rather
//! than a shortfall.
//!
//! A DNS blocker answers a name lookup with nothing. Chromium can be told to
//! stop asking it: DNS-over-HTTPS is on by default for many resolvers, and a
//! browser doing DoH resolves through an encrypted channel the blocker never
//! sees. So a DNS-level blocker is bypassed by a setting inside the thing it is
//! meant to be filtering.
//!
//! Our relay never resolves anything. SOCKS5 is used with SOCKS5h semantics —
//! the hostname travels to the upstream, which resolves it (`relay.rs` says why:
//! resolving locally would leak the operator's resolver). Which means the relay
//! is handed a NAME, in the CONNECT, before any connection exists.
//!
//! That is a better place to decide. Every byte the profile sends goes through
//! the relay by construction — `--proxy-bypass-list=<-loopback>` and no
//! exceptions — so a refusal here cannot be routed around by a browser setting,
//! by DoH, or by a hardcoded resolver.
//!
//! What it cannot do, and what a DNS blocker also cannot do: a connection made
//! to a literal IP address carries no name to match. Nothing here pretends
//! otherwise.
//!
//! ## Matching
//!
//! Blocking `doubleclick.net` blocks `ad.doubleclick.net`, because a tracker
//! that moved to a subdomain is the same tracker. It does NOT block
//! `notdoubleclick.net`, which is what a substring match would do and is the
//! classic way a blocklist takes down a site nobody meant to touch.
//!
//! So: exact match on the name, then on each parent label in turn. A name has
//! at most a handful of labels, so this is a handful of hash lookups rather
//! than a walk over a hundred thousand patterns.

use std::collections::HashSet;

/// A set of domains, and everything below them.
#[derive(Debug, Default, Clone)]
pub struct Blocklist {
    domains: HashSet<String>,
}

impl Blocklist {
    /// Reads the formats a published list actually comes in.
    ///
    /// Three of them, because the lists people already have are written three
    /// ways and asking somebody to convert theirs is asking them not to bother:
    ///
    /// ```text
    ///   0.0.0.0 ads.example       hosts file — the address is ignored
    ///   127.0.0.1 ads.example     the same, older style
    ///   ||ads.example^            Adblock Plus, the domain-anchored form
    ///   ads.example               one per line
    /// ```
    ///
    /// Anything else on a line is skipped rather than guessed at. An Adblock
    /// rule with a path, an element selector or an option is not a domain and
    /// pretending it is would block more than it says.
    pub fn parse(text: &str) -> Blocklist {
        let mut domains = HashSet::new();
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('!') {
                continue;
            }

            let candidate = if let Some(rest) = line.strip_prefix("||") {
                // ||domain^ or ||domain^$options — anything after the anchor is
                // a rule this does not implement, so the rule is skipped rather
                // than approximated.
                match rest.split_once('^') {
                    Some((domain, tail)) if tail.is_empty() => domain,
                    _ => continue,
                }
            } else {
                let mut fields = line.split_whitespace();
                let first = fields.next().unwrap_or("");
                match fields.next() {
                    // A hosts line: an address and then names.
                    Some(name) if is_address(first) => name,
                    // Two fields and the first is not an address: not a shape
                    // this understands.
                    Some(_) => continue,
                    None => first,
                }
            };

            if let Some(d) = normalise(candidate) {
                domains.insert(d);
            }
        }
        Blocklist { domains }
    }

    pub fn len(&self) -> usize {
        self.domains.len()
    }

    pub fn is_empty(&self) -> bool {
        self.domains.is_empty()
    }

    /// Is this host blocked, by itself or by one of its parents?
    pub fn blocks(&self, host: &str) -> bool {
        if self.domains.is_empty() {
            return false;
        }
        let Some(host) = normalise(host) else { return false };

        // The name, then each parent. `a.b.example.com` asks for
        // `a.b.example.com`, `b.example.com`, `example.com`, `com` — four
        // lookups, not a hundred thousand comparisons.
        let mut rest: &str = &host;
        loop {
            if self.domains.contains(rest) {
                return true;
            }
            match rest.split_once('.') {
                Some((_, parent)) if parent.contains('.') || !parent.is_empty() => rest = parent,
                _ => return false,
            }
        }
    }
}

fn is_address(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

/// Lowercased, with a trailing dot and any port removed. `None` for something
/// that is not a hostname at all.
fn normalise(raw: &str) -> Option<String> {
    let s = raw.trim().trim_end_matches('.');
    let s = s.split(':').next().unwrap_or(s);
    if s.is_empty() || !s.contains('.') {
        // A single label is either localhost or a typo, and blocking every
        // name that ends in it would be a wide net for no benefit.
        return None;
    }
    if s.parse::<std::net::IpAddr>().is_ok() {
        // "0.0.0.0" as the whole entry is a malformed hosts line, not a domain.
        return None;
    }
    if !s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'_') {
        return None;
    }
    Some(s.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_formats_a_published_list_comes_in() {
        let list = Blocklist::parse(
            "\
# a comment
0.0.0.0 ads.example.com
127.0.0.1  trackers.example.net
||beacon.example.org^
plain.example.io

! an adblock comment
",
        );
        assert_eq!(list.len(), 4, "{:?}", list.domains);
        for host in ["ads.example.com", "trackers.example.net", "beacon.example.org", "plain.example.io"] {
            assert!(list.blocks(host), "{host}");
        }
    }

    /// The behaviour that makes it useful, and the one that makes it safe.
    #[test]
    fn a_subdomain_is_blocked_and_a_lookalike_is_not() {
        let list = Blocklist::parse("doubleclick.net");
        assert!(list.blocks("doubleclick.net"));
        assert!(list.blocks("ad.doubleclick.net"));
        assert!(list.blocks("a.b.c.doubleclick.net"));

        // What a substring match would get wrong, and it is how a blocklist
        // takes down a site nobody meant to touch.
        assert!(!list.blocks("notdoubleclick.net"));
        assert!(!list.blocks("doubleclick.net.example.com"));
        assert!(!list.blocks("net"));
    }

    #[test]
    fn a_host_with_a_port_or_a_trailing_dot_still_matches() {
        let list = Blocklist::parse("ads.example.com");
        assert!(list.blocks("ads.example.com:443"));
        assert!(list.blocks("ADS.EXAMPLE.COM"));
        assert!(list.blocks("ads.example.com."));
    }

    /// Adblock rules that are not plain domains are skipped rather than
    /// approximated — an option or a path changes what the rule means.
    #[test]
    fn adblock_rules_that_are_not_domains_are_left_alone() {
        let list = Blocklist::parse(
            "\
||ads.example.com^
||tracker.example.com^$third-party
||example.com/ads/*
##.ad-banner
@@||allowed.example.com^
",
        );
        assert_eq!(list.len(), 1, "{:?}", list.domains);
        assert!(list.blocks("ads.example.com"));
        assert!(!list.blocks("tracker.example.com"), "an option changes the rule");
        assert!(!list.blocks("allowed.example.com"), "an exception is not a block");
    }

    #[test]
    fn an_empty_list_blocks_nothing_and_costs_nothing() {
        let list = Blocklist::default();
        assert!(list.is_empty());
        assert!(!list.blocks("anything.example.com"));
    }

    /// A literal address carries no name. Stated as a test so the limit is
    /// recorded rather than discovered.
    #[test]
    fn an_ip_literal_is_not_matched_by_any_domain_rule() {
        let list = Blocklist::parse("0.0.0.0 ads.example.com\nexample.com");
        assert!(!list.blocks("93.184.216.34"));
        // And a malformed hosts line whose only field is an address does not
        // become a domain entry.
        assert!(!list.blocks("0.0.0.0"));
    }

    #[test]
    fn a_hundred_thousand_entries_is_a_handful_of_lookups() {
        let text: String = (0..100_000).map(|i| format!("0.0.0.0 host{i}.example.com\n")).collect();
        let list = Blocklist::parse(&text);
        assert_eq!(list.len(), 100_000);

        let start = std::time::Instant::now();
        for i in 0..10_000 {
            assert!(list.blocks(&format!("sub.host{}.example.com", i % 100_000)));
        }
        // Not a benchmark — a guard against somebody replacing the label walk
        // with a scan over the set, which would be a hundred thousand string
        // comparisons per connection on the path every request takes.
        assert!(start.elapsed().as_secs() < 2, "took {:?}", start.elapsed());
    }
}
