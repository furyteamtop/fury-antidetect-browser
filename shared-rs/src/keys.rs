//! The key hierarchy, and the four operations it is made of.
//!
//! ```text
//! password ──Argon2id(kdf_salt)──► user key (UK)      never leaves this machine
//!                                    │ unwraps
//!                                    ▼
//!                              private key (X25519)   stored wrapped, server cannot read
//!                                    │ opens
//!                                    ▼
//!                       organisation root key (ORK)   sealed to the public key
//!                                    │ wraps
//!                                    ▼
//!            per-profile and per-credential data keys
//! ```
//!
//! Everything here runs on the client. The server stores `wrapped_private_key`,
//! `kdf_salt` and `wrapped_ork` and can do nothing with any of them — which is
//! the property the whole product is sold on, so it is worth being precise
//! about what it does and does not mean:
//!
//! - The server never sees the user key, the private key or the ORK.
//! - The server *does* see the login password, because it verifies it. A
//!   malicious server could therefore derive the user key at the moment someone
//!   signs in. Defeating that needs the password never to be sent at all,
//!   which is a change to the login protocol, not to this file. Until then the
//!   honest claim is "a stolen database is useless", not "a hostile operator
//!   cannot read your data".
//!
//! # Why sealing to a public key rather than to the user key
//!
//! Because membership changes. An admin who adds someone must be able to hand
//! them the ORK, and they do not have that person's password. Sealing to a
//! published X25519 key is what lets the organisation key be handed over by
//! someone who cannot impersonate the recipient.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

/// Argon2id parameters for deriving the user key from a password.
///
/// Deliberately heavier than the login hash: this one guards data that stays
/// valuable for years, and it runs once per sign-in on a laptop rather than on
/// every request on a shared server. 256 MiB and three passes is roughly a
/// second on the machines this ships to, and prices a GPU attack out of the
/// range where a memorable password is guessable.
pub const KDF_MEMORY_KIB: u32 = 262_144;
pub const KDF_PASSES: u32 = 3;
pub const KDF_LANES: u32 = 1;

pub const KEY_LEN: usize = 32;
pub const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("could not derive a key from the password: {0}")]
    Kdf(String),
    #[error("wrong password, or this data belongs to someone else")]
    Open,
    #[error("expected {expected} bytes, found {found}")]
    Length { expected: usize, found: usize },
    #[error("that public key cannot receive a sealed value")]
    UnusableKey,
}

/// A random salt, one per user, stored beside the wrapped key.
pub fn new_salt() -> [u8; SALT_LEN] {
    let mut s = [0u8; SALT_LEN];
    getrandom(&mut s);
    s
}

/// The user key: what a password becomes.
///
/// Never stored, never sent. Derived again on every sign-in, which is why the
/// cost above is a deliberate trade rather than an oversight.
pub fn user_key(password: &str, salt: &[u8]) -> Result<[u8; KEY_LEN], KeyError> {
    let params = argon2::Params::new(KDF_MEMORY_KIB, KDF_PASSES, KDF_LANES, Some(KEY_LEN))
        .map_err(|e| KeyError::Kdf(e.to_string()))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; KEY_LEN];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| KeyError::Kdf(e.to_string()))?;
    Ok(out)
}

/// Seal bytes under a symmetric key. Nonce first, then the ciphertext.
pub fn wrap(key: &[u8; KEY_LEN], plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom(&mut nonce);
    let cipher = XChaCha20Poly1305::new(key.into());
    let sealed = cipher
        .encrypt(XNonce::from_slice(&nonce), plain)
        // The only documented failure is a payload beyond what the AEAD can
        // address, which a 32-byte key is not.
        .expect("XChaCha20-Poly1305 cannot fail on a key-sized payload");
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    out
}

pub fn unwrap(key: &[u8; KEY_LEN], blob: &[u8]) -> Result<Vec<u8>, KeyError> {
    if blob.len() <= NONCE_LEN {
        return Err(KeyError::Length {
            expected: NONCE_LEN + 1,
            found: blob.len(),
        });
    }
    XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(&blob[..NONCE_LEN]), &blob[NONCE_LEN..])
        .map_err(|_| KeyError::Open)
}

/// An X25519 keypair. The private half is what a wrapped ORK opens under.
pub struct Keypair {
    pub public: [u8; KEY_LEN],
    pub secret: [u8; KEY_LEN],
}

pub fn new_keypair() -> Keypair {
    let mut secret = [0u8; KEY_LEN];
    getrandom(&mut secret);
    let sk = x25519_dalek::StaticSecret::from(secret);
    Keypair {
        public: x25519_dalek::PublicKey::from(&sk).to_bytes(),
        secret,
    }
}

/// Seal a payload to a public key, with no signature from the sender.
///
/// An ephemeral keypair per message, so the same ORK sealed twice to the same
/// member produces unrelated bytes, and nothing links the sealed copy back to
/// whoever sealed it. Layout: ephemeral public key, nonce, ciphertext.
///
/// The recipient's public key goes into the key derivation as well as the
/// exchange. Without it, a sealed blob could be replayed to a different
/// recipient by an attacker who could get them to accept an arbitrary
/// ephemeral key.
pub fn seal_to(recipient_public: &[u8; KEY_LEN], plain: &[u8]) -> Result<Vec<u8>, KeyError> {
    let mut eph = [0u8; KEY_LEN];
    getrandom(&mut eph);
    let eph_secret = x25519_dalek::StaticSecret::from(eph);
    let eph_public = x25519_dalek::PublicKey::from(&eph_secret).to_bytes();

    let shared = eph_secret.diffie_hellman(&x25519_dalek::PublicKey::from(*recipient_public));
    // X25519 has small-subgroup points whose exchange yields an all-zero shared
    // secret for *any* private key. A member who published one of those as
    // their public key would get an organisation key sealed under a value
    // anybody can compute — which is not a wrapped key at all. The recipient's
    // key arrives over the network from whoever is enrolling, so this is
    // checked rather than assumed.
    if !shared.was_contributory() {
        return Err(KeyError::UnusableKey);
    }
    let key = derive(shared.as_bytes(), &eph_public, recipient_public);

    let mut out = eph_public.to_vec();
    out.extend_from_slice(&wrap(&key, plain));
    Ok(out)
}

pub fn open_sealed(secret: &[u8; KEY_LEN], blob: &[u8]) -> Result<Vec<u8>, KeyError> {
    if blob.len() <= KEY_LEN + NONCE_LEN {
        return Err(KeyError::Length {
            expected: KEY_LEN + NONCE_LEN + 1,
            found: blob.len(),
        });
    }
    let mut eph_public = [0u8; KEY_LEN];
    eph_public.copy_from_slice(&blob[..KEY_LEN]);

    let sk = x25519_dalek::StaticSecret::from(*secret);
    let recipient_public = x25519_dalek::PublicKey::from(&sk).to_bytes();
    let shared = sk.diffie_hellman(&x25519_dalek::PublicKey::from(eph_public));
    // The same check on the way back: a blob carrying a small-subgroup
    // ephemeral key opens under a shared secret an attacker also knows, so it
    // is refused rather than decrypted.
    if !shared.was_contributory() {
        return Err(KeyError::UnusableKey);
    }
    let key = derive(shared.as_bytes(), &eph_public, &recipient_public);

    unwrap(&key, &blob[KEY_LEN..])
}

/// HKDF-SHA256 over the shared secret, bound to both public keys.
fn derive(shared: &[u8], eph_public: &[u8; KEY_LEN], recipient: &[u8; KEY_LEN]) -> [u8; KEY_LEN] {
    let mut salt = eph_public.to_vec();
    salt.extend_from_slice(recipient);
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), shared);
    let mut key = [0u8; KEY_LEN];
    hk.expand(b"fury-seal-v1", &mut key)
        .expect("32 bytes is within HKDF-SHA256's output range");
    key
}

/// A key for exactly one object, derived from the organisation key.
///
/// The organisation key opens every proxy credential and every profile in the
/// team. Handing it to a second process so that process can open *one* profile
/// is a bad trade, and the agent is exactly that case: it needs to unseal one
/// bundle, and it is the process that runs a browser full of other people's
/// JavaScript. So it gets a subkey — enough to open the profile it was asked
/// to launch, useless for the one next to it.
///
/// The id goes into the derivation after a zero byte, so that a label and an id
/// cannot be re-cut to produce the same input: ("ab", "c") and ("a", "bc") are
/// different keys.
pub fn subkey(ork: &[u8; KEY_LEN], label: &str, id: &str) -> [u8; KEY_LEN] {
    let mut info = label.as_bytes().to_vec();
    info.push(0);
    info.extend_from_slice(id.as_bytes());

    let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, ork);
    let mut out = [0u8; KEY_LEN];
    hk.expand(&info, &mut out)
        .expect("32 bytes is within HKDF-SHA256's output range");
    out
}

/// A fresh organisation root key.
pub fn new_org_key() -> [u8; KEY_LEN] {
    let mut k = [0u8; KEY_LEN];
    getrandom(&mut k);
    k
}

fn getrandom(buf: &mut [u8]) {
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Argon2id at 256 MiB is too slow to run in every test; the ones that only
    /// need *a* key use this instead.
    fn key(b: u8) -> [u8; KEY_LEN] {
        [b; KEY_LEN]
    }

    #[test]
    fn a_wrapped_value_opens_only_under_its_own_key() {
        let sealed = wrap(&key(1), b"the organisation key");
        assert_eq!(unwrap(&key(1), &sealed).unwrap(), b"the organisation key");
        assert!(matches!(unwrap(&key(2), &sealed), Err(KeyError::Open)));
    }

    #[test]
    fn wrapping_twice_gives_different_bytes() {
        // Per-message nonce. Without it, two members holding the same ORK would
        // be visibly the same row.
        assert_ne!(wrap(&key(1), b"same"), wrap(&key(1), b"same"));
    }

    #[test]
    fn a_sealed_key_opens_for_its_recipient_and_nobody_else() {
        let alice = new_keypair();
        let bob = new_keypair();
        let ork = new_org_key();

        let sealed = seal_to(&alice.public, &ork).unwrap();
        assert_eq!(open_sealed(&alice.secret, &sealed).unwrap(), ork.to_vec());
        // Bob holds a valid key, and it is the wrong one.
        assert!(open_sealed(&bob.secret, &sealed).is_err());
    }

    #[test]
    fn sealing_reveals_neither_the_payload_nor_a_repeat() {
        let alice = new_keypair();
        let ork = new_org_key();
        let once = seal_to(&alice.public, &ork).unwrap();
        let twice = seal_to(&alice.public, &ork).unwrap();
        assert_ne!(once, twice, "sealing is deterministic — the ephemeral key is not ephemeral");
        assert!(
            !once.windows(KEY_LEN).any(|w| w == ork),
            "the organisation key survived in the clear"
        );
    }

    #[test]
    fn a_key_that_cannot_receive_a_secret_is_refused() {
        // Small-subgroup points: the exchange gives an all-zero shared secret
        // for every private key, so anything "sealed" to one is readable by
        // anybody. These arrive from whoever is enrolling.
        for bad in [[0u8; KEY_LEN], {
            let mut one = [0u8; KEY_LEN];
            one[0] = 1;
            one
        }] {
            assert!(
                matches!(seal_to(&bad, b"the organisation key"), Err(KeyError::UnusableKey)),
                "a degenerate public key was accepted"
            );
        }
        // And an ordinary key still works, so the check is not simply refusing.
        assert!(seal_to(&new_keypair().public, b"x").is_ok());
    }

    #[test]
    fn a_subkey_opens_one_thing_and_not_its_neighbour() {
        let ork = new_org_key();
        let a = subkey(&ork, "fury-profile-v1", "profile-a");
        let b = subkey(&ork, "fury-profile-v1", "profile-b");

        assert_ne!(a, b, "one subkey for two profiles");
        assert_eq!(a, subkey(&ork, "fury-profile-v1", "profile-a"), "not stable");
        assert_ne!(a, ork, "the subkey is the organisation key");

        // A different organisation cannot reach it either.
        assert_ne!(a, subkey(&new_org_key(), "fury-profile-v1", "profile-a"));

        // The zero byte: a label and an id must not be re-cuttable.
        assert_ne!(subkey(&ork, "ab", "c"), subkey(&ork, "a", "bc"));
    }

    #[test]
    fn a_subkey_cannot_be_walked_back_to_the_organisation_key() {
        // Not a proof, a guard: the derived bytes must not contain the input.
        let ork = new_org_key();
        let sub = subkey(&ork, "fury-profile-v1", "p");
        assert!(!sub.windows(8).any(|w| ork.windows(8).any(|o| o == w)));
    }

    #[test]
    fn a_truncated_blob_is_an_error_rather_than_a_panic() {
        let alice = new_keypair();
        let sealed = seal_to(&alice.public, b"x").unwrap();
        for cut in [0, 1, KEY_LEN, KEY_LEN + NONCE_LEN] {
            assert!(open_sealed(&alice.secret, &sealed[..cut]).is_err(), "cut at {cut}");
        }
        assert!(unwrap(&key(1), &[]).is_err());
    }

    #[test]
    fn a_tampered_blob_does_not_open() {
        let alice = new_keypair();
        let mut sealed = seal_to(&alice.public, b"the organisation key").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        assert!(open_sealed(&alice.secret, &sealed).is_err());
    }

    #[test]
    fn the_same_password_and_salt_give_the_same_key() {
        let salt = new_salt();
        let a = user_key("correct horse battery staple", &salt).unwrap();
        let b = user_key("correct horse battery staple", &salt).unwrap();
        assert_eq!(a, b);

        // A different salt is a different key: two people who choose the same
        // password do not share one.
        let c = user_key("correct horse battery staple", &new_salt()).unwrap();
        assert_ne!(a, c);

        let d = user_key("correct horse battery stapl", &salt).unwrap();
        assert_ne!(a, d);
    }

    #[test]
    fn the_whole_chain_holds() {
        // Exactly what enrolment does, and then what signing in on a second
        // machine does.
        let password = "a password nobody else has";
        let salt = new_salt();
        let uk = user_key(password, &salt).unwrap();
        let kp = new_keypair();
        let wrapped_private = wrap(&uk, &kp.secret);
        let ork = new_org_key();
        let wrapped_ork = seal_to(&kp.public, &ork).unwrap();

        // The server holds: salt, wrapped_private, wrapped_ork, public key.
        // Somewhere else entirely, with only the password:
        let uk2 = user_key(password, &salt).unwrap();
        let secret: [u8; KEY_LEN] = unwrap(&uk2, &wrapped_private).unwrap().try_into().unwrap();
        let recovered = open_sealed(&secret, &wrapped_ork).unwrap();
        assert_eq!(recovered, ork.to_vec());

        // And with the wrong password, nothing at all.
        let wrong = user_key("a password nobody else has ", &salt).unwrap();
        assert!(unwrap(&wrong, &wrapped_private).is_err());
    }
}
