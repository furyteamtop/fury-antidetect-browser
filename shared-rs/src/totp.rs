// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Time-based one-time passwords — RFC 6238, and what a shared account needs
//! from them.
//!
//! # Why this is in the product at all
//!
//! A team account with two-factor authentication has a seed, and that seed has
//! to live somewhere every member can reach. In practice it lives in a group
//! chat, or in one person's phone, and both are worse than they sound: the chat
//! is forever and searchable, and the phone is a person who has to be awake.
//!
//! So it lives with the profile, sealed, and the code is generated on the
//! operator's own machine when they ask for it.
//!
//! # What is deliberately not here
//!
//! No autofill into the page. A browser that types a one-time code into a form
//! by itself is a browser doing something no human does at that speed, and the
//! timing of it is measurable. The code is shown, and a person pastes it.
//!
//! # Why the implementation and not a crate
//!
//! Sixty lines against a dependency that pulls its own base32 and its own HMAC,
//! in a binary where every dependency is one more thing to audit for a product
//! whose whole claim is that nothing phones home. The test vectors from RFC 6238
//! appendix B are below, all of them, for all three hashes.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};

/// Which hash an authenticator was set up with.
///
/// Almost everything in the world is SHA-1 and always will be: the algorithm
/// travels in the otpauth:// URI, and an issuer that picked SHA-256 will send
/// it, so guessing is never necessary. Present because a seed set up with one
/// and read with another produces six plausible digits that are simply wrong,
/// and the operator's report is "the site says the code is invalid".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Algorithm {
    #[default]
    Sha1,
    Sha256,
    Sha512,
}

impl Algorithm {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().replace('-', "").as_str() {
            "SHA1" => Some(Self::Sha1),
            "SHA256" => Some(Self::Sha256),
            "SHA512" => Some(Self::Sha512),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Sha1 => "SHA1",
            Self::Sha256 => "SHA256",
            Self::Sha512 => "SHA512",
        }
    }
}

/// Three concrete arms rather than one generic function.
///
/// The generic version needs four trait bounds spelling out that `Hmac<D>` can
/// be keyed and finalised, which is more code than the repetition and reads
/// worse. Three hashes is the whole set the RFC defines.
macro_rules! hmac_bytes {
    ($hash:ty, $key:expr, $counter:expr) => {{
        let mut mac = <Hmac<$hash> as Mac>::new_from_slice($key)
            .expect("HMAC accepts a key of any length");
        mac.update(&$counter.to_be_bytes());
        mac.finalize().into_bytes().to_vec()
    }};
}

/// A configured generator: the seed and the parameters it was set up with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Totp {
    pub secret: Vec<u8>,
    pub digits: u32,
    pub period: u64,
    pub algorithm: Algorithm,
    /// What the authenticator app would have shown as the account name, if the
    /// seed arrived as a URI. Kept so an operator pasting three of them can tell
    /// which is which.
    pub label: Option<String>,
    pub issuer: Option<String>,
}

impl Totp {
    /// From a bare base32 secret, with the defaults every authenticator uses.
    pub fn from_secret(secret: &str) -> Result<Self, Error> {
        Ok(Self {
            secret: base32_decode(secret).ok_or(Error::NotBase32)?,
            digits: 6,
            period: 30,
            algorithm: Algorithm::Sha1,
            label: None,
            issuer: None,
        })
    }

    /// From whatever the operator pasted.
    ///
    /// Sites hand out the seed in three shapes and people paste all three: the
    /// `otpauth://` URI behind the QR code, the bare base32 string beside it,
    /// and that string with the spaces the site put in for readability. Refusing
    /// two of the three would be refusing the thing most people have.
    pub fn parse(input: &str) -> Result<Self, Error> {
        let input = input.trim();
        if input.is_empty() {
            return Err(Error::Empty);
        }
        if input.to_ascii_lowercase().starts_with("otpauth://") {
            Self::from_uri(input)
        } else {
            Self::from_secret(input)
        }
    }

    fn from_uri(uri: &str) -> Result<Self, Error> {
        let rest = &uri["otpauth://".len()..];
        let (kind, rest) = rest.split_once('/').ok_or(Error::BadUri)?;
        if !kind.eq_ignore_ascii_case("totp") {
            // hotp counts events rather than seconds. Refused rather than
            // treated as totp, which would produce six digits that are never
            // right and no clue why.
            return Err(Error::NotTotp);
        }
        let (label, query) = match rest.split_once('?') {
            Some((l, q)) => (l, q),
            None => (rest, ""),
        };

        let mut secret = None;
        let mut digits = 6;
        let mut period = 30;
        let mut algorithm = Algorithm::Sha1;
        let mut issuer = None;
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            let v = percent_decode(v);
            match k.to_ascii_lowercase().as_str() {
                "secret" => secret = base32_decode(&v),
                "digits" => digits = v.parse().unwrap_or(6),
                "period" => period = v.parse().unwrap_or(30),
                "algorithm" => algorithm = Algorithm::parse(&v).unwrap_or(Algorithm::Sha1),
                "issuer" => issuer = Some(v),
                _ => {}
            }
        }

        let label = percent_decode(label);
        // "Issuer:account" is the conventional label, and the issuer in it is
        // the one to trust when the query parameter disagrees — that is what
        // the authenticators do.
        let (from_label, account) = match label.split_once(':') {
            Some((i, a)) => (Some(i.trim().to_string()), a.trim().to_string()),
            None => (None, label.clone()),
        };

        Ok(Self {
            secret: secret.ok_or(Error::NoSecret)?,
            digits: digits.clamp(6, 10),
            period: period.max(1),
            algorithm,
            label: (!account.is_empty()).then_some(account),
            issuer: from_label.or(issuer),
        })
    }

    /// The code for a given moment, as seconds since the epoch.
    ///
    /// Takes the time rather than reading the clock, so the RFC's vectors can be
    /// checked and so a caller can show the next code as well as this one.
    pub fn code_at(&self, unix_seconds: u64) -> String {
        let counter = unix_seconds / self.period;
        let digest = match self.algorithm {
            Algorithm::Sha1 => hmac_bytes!(Sha1, &self.secret, counter),
            Algorithm::Sha256 => hmac_bytes!(Sha256, &self.secret, counter),
            Algorithm::Sha512 => hmac_bytes!(Sha512, &self.secret, counter),
        };

        // Dynamic truncation, RFC 4226 section 5.3. The low nibble of the last
        // byte picks where to read four bytes from; the high bit is masked off
        // because the result is read as a signed integer in the reference
        // implementation and a negative modulus is not a code.
        let offset = (digest[digest.len() - 1] & 0x0f) as usize;
        let binary = u32::from_be_bytes([
            digest[offset] & 0x7f,
            digest[offset + 1],
            digest[offset + 2],
            digest[offset + 3],
        ]);
        let modulus = 10u32.pow(self.digits);
        format!("{:0width$}", binary % modulus, width = self.digits as usize)
    }

    /// Back to an `otpauth://` URI, so the parameters travel with the secret.
    ///
    /// Everything is stored in this shape rather than as the operator pasted
    /// it. A bare base32 string carries no digits, no period and no hash, so
    /// keeping it as typed means guessing those at every read — and the guess
    /// is right almost always, which is the worst kind of guess: it works for
    /// months and then silently produces wrong codes for the one account whose
    /// issuer chose SHA-256.
    pub fn to_uri(&self) -> String {
        let label = self.label.as_deref().unwrap_or("account");
        let mut uri = format!(
            "otpauth://totp/{}?secret={}&algorithm={}&digits={}&period={}",
            percent_encode(label),
            base32_encode(&self.secret),
            self.algorithm.as_str(),
            self.digits,
            self.period,
        );
        if let Some(issuer) = &self.issuer {
            uri.push_str("&issuer=");
            uri.push_str(&percent_encode(issuer));
        }
        uri
    }

    /// Seconds until this code is replaced.
    ///
    /// Shown beside the code, because a one-time password handed over with two
    /// seconds left is a login that fails for a reason nobody attributes to the
    /// clock.
    pub fn seconds_remaining(&self, unix_seconds: u64) -> u64 {
        self.period - (unix_seconds % self.period)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Empty,
    NotBase32,
    BadUri,
    NotTotp,
    NoSecret,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Written for the person who pasted something, not for a log.
        let s = match self {
            Self::Empty => "nothing to read",
            Self::NotBase32 => {
                "this is not a base32 secret. Paste the key the site showed beside \
                 the QR code, or the whole otpauth:// link"
            }
            Self::BadUri => "this otpauth:// link is malformed",
            Self::NotTotp => {
                "this is an otpauth://hotp link, which counts logins rather than \
                 seconds. Fury generates time-based codes only"
            }
            Self::NoSecret => "this otpauth:// link carries no secret",
        };
        f.write_str(s)
    }
}

impl std::error::Error for Error {}


/// RFC 4648 base32, without padding and case-insensitively.
///
/// Written out rather than pulled in: sites print the secret in groups of four
/// with spaces, in lower case, sometimes with the `=` padding and usually
/// without, and a decoder that accepts only one of those shapes rejects what
/// most people have in front of them.
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0u32;
    let mut nbits = 0u32;
    let mut out = Vec::new();
    for c in s.bytes() {
        if c == b' ' || c == b'-' || c == b'=' || c == b'\n' || c == b'\r' || c == b'\t' {
            continue;
        }
        let c = c.to_ascii_uppercase();
        let v = ALPHABET.iter().position(|a| *a == c)? as u32;
        bits = (bits << 5) | v;
        nbits += 5;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// RFC 4648 base32, unpadded — the shape every authenticator prints.
fn base32_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut bits = 0u32;
    let mut nbits = 0u32;
    for b in bytes {
        bits = (bits << 8) | u32::from(*b);
        nbits += 8;
        while nbits >= 5 {
            nbits -= 5;
            out.push(ALPHABET[((bits >> nbits) & 0x1f) as usize] as char);
        }
    }
    if nbits > 0 {
        out.push(ALPHABET[((bits << (5 - nbits)) & 0x1f) as usize] as char);
    }
    out
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'@') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 appendix B, in full and for all three hashes.
    ///
    /// The whole table rather than a couple of rows: this is the only thing
    /// standing between a shared account and six digits that look right, and an
    /// implementation that passes SHA-1 and fails SHA-512 fails silently for
    /// exactly the people who chose the stronger option.
    #[test]
    fn rfc6238_test_vectors() {
        // The RFC's seed is "12345678901234567890" repeated to the hash's block
        // size, given there as ASCII.
        let s1 = b"12345678901234567890".to_vec();
        let s256 = b"12345678901234567890123456789012".to_vec();
        let s512 = b"1234567890123456789012345678901234567890123456789012345678901234".to_vec();

        let cases: &[(u64, &str, &str, &str)] = &[
            (59, "94287082", "46119246", "90693936"),
            (1111111109, "07081804", "68084774", "25091201"),
            (1111111111, "14050471", "67062674", "99943326"),
            (1234567890, "89005924", "91819424", "93441116"),
            (2000000000, "69279037", "90698825", "38618901"),
            (20000000000, "65353130", "77737706", "47863826"),
        ];

        for (t, sha1, sha256, sha512) in cases {
            for (secret, algorithm, want) in [
                (&s1, Algorithm::Sha1, sha1),
                (&s256, Algorithm::Sha256, sha256),
                (&s512, Algorithm::Sha512, sha512),
            ] {
                let totp = Totp {
                    secret: secret.clone(),
                    digits: 8,
                    period: 30,
                    algorithm,
                    label: None,
                    issuer: None,
                };
                assert_eq!(
                    &totp.code_at(*t),
                    want,
                    "RFC 6238 vector t={t} {}",
                    algorithm.as_str()
                );
            }
        }
    }

    #[test]
    fn the_three_shapes_people_actually_paste() {
        // The URI behind the QR code.
        let uri = Totp::parse(
            "otpauth://totp/ACME%20Co:alice@example.com?secret=JBSWY3DPEHPK3PXP&issuer=ACME%20Co&digits=6&period=30",
        )
        .unwrap();
        assert_eq!(uri.digits, 6);
        assert_eq!(uri.label.as_deref(), Some("alice@example.com"));
        assert_eq!(uri.issuer.as_deref(), Some("ACME Co"));

        // The bare secret beside it.
        let bare = Totp::parse("JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(bare.secret, uri.secret);

        // And the same with the spaces the site printed for readability, in
        // lower case, which is what a person copying by eye produces.
        let spaced = Totp::parse("jbsw y3dp ehpk 3pxp").unwrap();
        assert_eq!(spaced.secret, uri.secret);
    }

    #[test]
    fn an_hotp_link_is_refused_rather_than_read_as_totp() {
        // Both start otpauth:// and both carry a base32 secret, so reading one
        // as the other produces six plausible digits that are never accepted,
        // and nothing points at the cause.
        let err = Totp::parse("otpauth://hotp/x?secret=JBSWY3DPEHPK3PXP&counter=1").unwrap_err();
        assert_eq!(err, Error::NotTotp);
        assert!(err.to_string().contains("time-based"));
    }

    #[test]
    fn nonsense_says_what_to_paste_instead() {
        let err = Totp::parse("hunter2!!").unwrap_err();
        assert_eq!(err, Error::NotBase32);
        assert!(err.to_string().contains("beside the QR code"), "{err}");
    }

    #[test]
    fn the_countdown_reaches_zero_exactly_when_the_code_changes() {
        // Shown beside the code: a one-time password handed to a colleague with
        // two seconds left is a login that fails for a reason nobody attributes
        // to the clock.
        let t = Totp::from_secret("JBSWY3DPEHPK3PXP").unwrap();
        for now in [0u64, 1, 29, 30, 31, 59, 60] {
            let left = t.seconds_remaining(now);
            assert!(left >= 1 && left <= 30, "at {now}: {left}");
            assert_eq!(
                t.code_at(now),
                t.code_at(now + left - 1),
                "the code changed before its countdown ran out"
            );
            assert_ne!(
                t.code_at(now),
                t.code_at(now + left),
                "the code outlived its countdown"
            );
        }
    }

    #[test]
    fn a_round_trip_through_the_uri_keeps_every_parameter() {
        // The reason storage normalises to a URI: everything that decides what
        // the six digits are has to survive being written down.
        for original in [
            "otpauth://totp/ACME:alice@example.com?secret=JBSWY3DPEHPK3PXP&issuer=ACME&algorithm=SHA256&digits=8&period=60",
            "otpauth://totp/plain?secret=JBSWY3DPEHPK3PXP",
        ] {
            let a = Totp::parse(original).unwrap();
            let b = Totp::parse(&a.to_uri()).unwrap();
            assert_eq!(a, b, "a parameter was lost writing {original}");
            assert_eq!(a.code_at(1234567890), b.code_at(1234567890));
        }
    }

    #[test]
    fn a_bare_secret_becomes_a_uri_that_still_produces_the_same_code() {
        // What an operator pastes most often, and the shape it is kept in.
        let pasted = Totp::parse("jbsw y3dp ehpk 3pxp").unwrap();
        let stored = Totp::parse(&pasted.to_uri()).unwrap();
        assert_eq!(pasted.secret, stored.secret);
        assert_eq!(pasted.code_at(59), stored.code_at(59));
    }

    #[test]
    fn a_seed_read_with_the_wrong_hash_is_wrong_rather_than_close() {
        // Why Algorithm exists at all. The failure is silent: six digits, the
        // right shape, rejected by the site, and no clue in any message.
        let secret = base32_decode("JBSWY3DPEHPK3PXP").unwrap();
        let mk = |a| Totp { secret: secret.clone(), digits: 6, period: 30, algorithm: a, label: None, issuer: None };
        assert_ne!(mk(Algorithm::Sha1).code_at(59), mk(Algorithm::Sha256).code_at(59));
        assert_ne!(mk(Algorithm::Sha1).code_at(59), mk(Algorithm::Sha512).code_at(59));
    }
}
