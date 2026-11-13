// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The account recovery key: a second lock on the private key, held only by
//! the person it belongs to (docs/25).
//!
//! ```text
//! code (256 bits) ──Argon2id(recovery_salt)──HKDF "wrap"──► key that wraps the private key
//!                 └─────────────────────────HKDF "auth"──► secret the server checks a hash of
//! ```
//!
//! The server stores the wrapped private key, the salt, and an Argon2id hash
//! of the "auth" secret. From those it can tell a right code from a wrong one
//! and nothing else: the wrapping key is a different derivation of the code,
//! and the code is 256 random bits nobody can guess.
//!
//! "auth" takes no salt on purpose. The salt lives on the server, and a client
//! that needed it before its code was checked would have to be given it for
//! any address it asked about -- which would tell anyone which accounts have a
//! recovery key. HKDF over 256 random bits needs no salt to be unguessable.
//!
//! Argon2id on the wrapping side buys no brute-force resistance a random
//! 256-bit input does not already have. It is there so that the code and the
//! password go through the same reviewed derivation, and so that a code with
//! less entropy than intended (a future change, a bug in `new_code`) is still
//! expensive to search offline against a stolen wrapping.

use crate::keys::{self, KeyError, KEY_LEN};

/// Raw bytes in a code.
pub const CODE_BYTES: usize = 32;
/// Characters in the printed form, without separators: 256 bits in base32.
pub const CODE_CHARS: usize = 52;

/// Crockford's alphabet, lower case: no I, L, O or U, so nothing on paper reads
/// as a 1, a 0 or a word. The same alphabet as invitation codes.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodeError {
    #[error("a recovery key has {CODE_CHARS} characters; this has {0}")]
    Length(usize),
    #[error("{0:?} is not a character a recovery key uses")]
    Character(char),
    #[error("this is not a recovery key Fury made")]
    Padding,
}

/// A fresh code: the raw bytes and the form shown to a person.
pub fn new_code() -> ([u8; CODE_BYTES], String) {
    let raw = keys::new_org_key();
    let printed = format(&raw);
    (raw, printed)
}

/// `abcd-efgh-...`: thirteen groups of four.
pub fn format(raw: &[u8; CODE_BYTES]) -> String {
    let mut chars = Vec::with_capacity(CODE_CHARS);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in raw {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            chars.push(ALPHABET[((acc >> bits) & 0x1f) as usize]);
        }
    }
    // 256 bits leave one 1-bit tail, padded with zeros to a whole symbol.
    if bits > 0 {
        chars.push(ALPHABET[((acc << (5 - bits)) & 0x1f) as usize]);
    }
    chars
        .chunks(4)
        .map(|g| std::str::from_utf8(g).expect("alphabet is ASCII"))
        .collect::<Vec<_>>()
        .join("-")
}

/// The code as typed back: separators, spaces and case ignored, and the
/// letters Crockford maps to digits (I and L to 1, O to 0) accepted, because a
/// code copied by hand from paper will contain them.
pub fn parse(text: &str) -> Result<[u8; CODE_BYTES], CodeError> {
    let mut symbols = Vec::with_capacity(CODE_CHARS);
    for c in text.chars() {
        if c.is_whitespace() || c == '-' || c == '_' {
            continue;
        }
        let c = c.to_ascii_lowercase();
        let c = match c {
            'i' | 'l' => '1',
            'o' => '0',
            other => other,
        };
        let v = ALPHABET
            .iter()
            .position(|&a| a as char == c)
            .ok_or(CodeError::Character(c))?;
        symbols.push(v as u32);
    }
    if symbols.len() != CODE_CHARS {
        return Err(CodeError::Length(symbols.len()));
    }
    let mut out = [0u8; CODE_BYTES];
    let (mut acc, mut bits, mut i) = (0u32, 0u32, 0usize);
    for v in symbols {
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            if i < CODE_BYTES {
                out[i] = (acc >> bits) as u8;
                i += 1;
            }
        }
        acc &= (1 << bits) - 1;
    }
    // The tail must be the zero padding `format` wrote; anything else is a
    // mistyped last character that happens to be in the alphabet.
    if acc != 0 {
        return Err(CodeError::Padding);
    }
    Ok(out)
}

/// The two keys a code becomes, under one salt.
pub struct Derived {
    /// Wraps the private key. Never leaves the client.
    pub wrap: [u8; KEY_LEN],
    /// Sent to the server, which keeps only an Argon2id hash of it.
    pub auth: [u8; KEY_LEN],
}

pub fn derive(raw: &[u8; CODE_BYTES], salt: &[u8]) -> Result<Derived, KeyError> {
    let params = argon2::Params::new(keys::KDF_MEMORY_KIB, keys::KDF_PASSES, keys::KDF_LANES, Some(KEY_LEN))
        .map_err(|e| KeyError::Kdf(e.to_string()))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut seed = [0u8; KEY_LEN];
    argon
        .hash_password_into(raw, salt, &mut seed)
        .map_err(|e| KeyError::Kdf(e.to_string()))?;
    Ok(Derived { wrap: hkdf32(&seed, b"fury recovery wrap v1"), auth: auth(raw) })
}

/// The half the server checks. From the code alone, so it can be sent before
/// the server has said anything about the account (see the module comment).
pub fn auth(raw: &[u8; CODE_BYTES]) -> [u8; KEY_LEN] {
    hkdf32(raw, b"fury recovery auth v1")
}

fn hkdf32(ikm: &[u8], info: &[u8]) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    hkdf::Hkdf::<sha2::Sha256>::new(None, ikm)
        .expand(info, &mut out)
        .expect("32 bytes is within HKDF range");
    out
}

/// What goes to the server when a code is made.
pub struct Material {
    pub salt: [u8; keys::SALT_LEN],
    pub wrapped_private_key: Vec<u8>,
    pub auth: [u8; KEY_LEN],
}

/// Lock `private_key` under `raw`.
pub fn seal(raw: &[u8; CODE_BYTES], private_key: &[u8; KEY_LEN]) -> Result<Material, KeyError> {
    let salt = keys::new_salt();
    let d = derive(raw, &salt)?;
    Ok(Material { salt, wrapped_private_key: keys::wrap(&d.wrap, private_key), auth: d.auth })
}

/// Open the private key with a code, and refuse it unless it is the key whose
/// public half the server publishes for this account. Without that check a
/// server could hand back a key of its own choosing, and the client would wrap
/// it under the new password and seal itself to the server.
pub fn open(
    d: &Derived,
    wrapped_private_key: &[u8],
    expected_public: &[u8; KEY_LEN],
) -> Result<[u8; KEY_LEN], KeyError> {
    let plain = keys::unwrap(&d.wrap, wrapped_private_key)?;
    let secret: [u8; KEY_LEN] = plain
        .as_slice()
        .try_into()
        .map_err(|_| KeyError::Length { expected: KEY_LEN, found: plain.len() })?;
    if &keys::public_from_secret(&secret) != expected_public {
        return Err(KeyError::Open);
    }
    Ok(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_prints_as_thirteen_groups_and_reads_back() {
        for _ in 0..50 {
            let (raw, printed) = new_code();
            assert_eq!(printed.len(), CODE_CHARS + 12, "{printed}");
            assert_eq!(printed.split('-').count(), 13);
            assert_eq!(parse(&printed).unwrap(), raw);
        }
    }

    #[test]
    fn a_code_typed_by_hand_still_reads() {
        let (raw, printed) = new_code();
        let sloppy = printed.to_uppercase().replace('-', " ").replace('1', "l").replace('0', "O");
        assert_eq!(parse(&sloppy).unwrap(), raw);
    }

    #[test]
    fn a_wrong_length_or_character_or_tail_is_refused() {
        let (_, printed) = new_code();
        assert!(matches!(parse(&printed[..printed.len() - 1]), Err(CodeError::Length(_))));
        assert_eq!(parse(&format!("{printed}u")), Err(CodeError::Character('u')));
        // The last symbol carries one real bit and four bits of padding.
        let mut last = printed.clone();
        let tail = last.pop().unwrap();
        let bad = if tail == '0' || tail == 'g' { '1' } else { '2' };
        last.push(bad);
        assert_eq!(parse(&last), Err(CodeError::Padding));
    }

    #[test]
    fn the_server_half_differs_from_the_wrapping_side_and_from_the_code() {
        let raw = [1u8; CODE_BYTES];
        assert_ne!(auth(&raw), raw);
        assert_ne!(auth(&raw), hkdf32(&raw, b"fury recovery wrap v1"));
        assert_ne!(auth(&raw), auth(&[2u8; CODE_BYTES]));
    }

    #[test]
    fn the_private_key_comes_back_only_as_the_published_one() {
        // One real Argon2id derivation: slow, so only here.
        let pair = keys::new_keypair();
        let (raw, _) = new_code();
        let m = seal(&raw, &pair.secret).unwrap();
        let d = derive(&raw, &m.salt).unwrap();
        assert_eq!(d.auth, m.auth);
        assert_eq!(open(&d, &m.wrapped_private_key, &pair.public).unwrap(), pair.secret);

        // A server serving some other key under this code is caught.
        let other = keys::new_keypair();
        let swapped = keys::wrap(&d.wrap, &other.secret);
        assert!(open(&d, &swapped, &pair.public).is_err());
        // And a wrong code opens nothing.
        let wrong = Derived { wrap: hkdf32(&[9u8; 32], b"fury recovery wrap v1"), auth: auth(&[9u8; 32]) };
        assert!(open(&wrong, &m.wrapped_private_key, &pair.public).is_err());
    }
}
