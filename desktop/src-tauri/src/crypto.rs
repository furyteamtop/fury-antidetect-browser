// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Where the password becomes keys.
//!
//! Thin: the primitives live in `fury_shared::keys`, so the agent and the shell
//! cannot drift into two incompatible notions of what a wrapped key is. What
//! this file adds is the base64 boundary with the server and the rule about
//! where the results are allowed to live.
//!
//! **Nothing here returns key material to the webview.** The unwrapped
//! organisation key stays in `AppState`, for the same reason lock tokens do:
//! anything handed to the front end is one `devtools` away from being read, and
//! the ORK opens every proxy credential in the organisation. The webview asks
//! for *effects* — decrypt this proxy, seal this bundle — and gets results.

use fury_shared::keys;

use base64::Engine;

fn b64(v: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(v)
}

fn unb64(what: &str, v: &str) -> anyhow::Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(v)
        .map_err(|_| anyhow::anyhow!("{what} from the server is not base64"))
}

fn key32(what: &str, v: Vec<u8>) -> anyhow::Result<[u8; 32]> {
    v.try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("{what} is {} bytes, expected 32", v.len()))
}

/// Everything a new account is made of, generated here and nowhere else.
pub struct Enrolment {
    pub public_key: String,
    pub wrapped_private_key: String,
    pub kdf_salt: String,
    /// Present only when this enrolment creates the organisation.
    pub wrapped_ork: Option<String>,
    /// The organisation key in the clear, to be held in memory and never sent.
    pub org_key: Option<[u8; 32]>,
}

/// Generate a keypair, wrap it under the password, and — for an owner — bring
/// the organisation key into being.
///
/// `creates_org` comes from the invitation, not from the person enrolling: a
/// member who decided for themselves that they were an owner would generate a
/// second organisation key, and the resulting data would be readable by nobody
/// but them.
pub fn enrol(password: &str, creates_org: bool) -> anyhow::Result<Enrolment> {
    let salt = keys::new_salt();
    let uk = keys::user_key(password, &salt)?;
    let kp = keys::new_keypair();
    let wrapped_private_key = keys::wrap(&uk, &kp.secret);

    let (wrapped_ork, org_key) = if creates_org {
        let ork = keys::new_org_key();
        // Sealed to our own public key rather than wrapped under the user key:
        // it makes the owner's copy the same shape as every member's, so
        // handing the key to a second person later is not a special case.
        (Some(b64(&keys::seal_to(&kp.public, &ork)?)), Some(ork))
    } else {
        (None, None)
    };

    Ok(Enrolment {
        public_key: b64(&kp.public),
        wrapped_private_key: b64(&wrapped_private_key),
        kdf_salt: b64(&salt),
        wrapped_ork,
        org_key,
    })
}

/// Eight digits from a member's public key, for reading out to whoever lets
/// them in.
///
/// Why it exists: the key is handed over to the public key the SERVER lists
/// for a member, and an invitation code works for whoever types it first. A
/// code that leaked enrolled a stranger as the invitee, and nothing in Users
/// told the two apart (14.10.2026). The newcomer's app computes these digits
/// from the key it generated itself and keeps; the owner's from the key the
/// server lists. The same digits mean the person on the other end of the
/// conversation is the one whose key is about to be trusted, and that the
/// server did not swap it on the way.
///
/// SHA-256 under a label, first eight bytes, eight decimal digits: 27 bits,
/// plenty for a comparison an attacker cannot repeat (they would need a key
/// matching digits they do not know, before the owner asks).
pub fn verify_code(public_key: &str) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    let pk = key32("the public key", unb64("public_key", public_key)?)?;
    let mut h = Sha256::new();
    h.update(b"fury verify code v1\0");
    h.update(pk);
    let d = h.finalize();
    let n = u64::from_be_bytes(d[..8].try_into().expect("eight bytes")) % 100_000_000;
    Ok(format!("{:04}-{:04}", n / 10_000, n % 10_000))
}

/// One's own public key, from the password, at a sign-in that holds no
/// organisation key yet: the moment `verify_code` is needed on this side, and
/// the only one at which the private key can be opened.
pub fn own_public_key(password: &str, kdf_salt: &str, wrapped_private_key: &str) -> anyhow::Result<String> {
    let salt = unb64("kdf_salt", kdf_salt)?;
    let uk = keys::user_key(password, &salt)?;
    let secret = key32(
        "the wrapped private key",
        keys::unwrap(&uk, &unb64("wrapped_private_key", wrapped_private_key)?)?,
    )?;
    Ok(b64(&keys::public_from_secret(&secret)))
}

/// Recover the organisation key at sign-in.
///
/// Returns `Ok(None)` for an account that has enrolled but not yet been handed
/// the key — a real state, not an error: they can sign in and see the team, and
/// decrypt nothing, until an admin wraps the key to their public key.
pub fn unlock_org_key(
    password: &str,
    kdf_salt: &str,
    wrapped_private_key: &str,
    wrapped_ork: Option<&str>,
) -> anyhow::Result<Option<[u8; 32]>> {
    let Some(wrapped_ork) = wrapped_ork else {
        return Ok(None);
    };

    let salt = unb64("kdf_salt", kdf_salt)?;
    let uk = keys::user_key(password, &salt)?;
    let secret = key32(
        "the wrapped private key",
        keys::unwrap(&uk, &unb64("wrapped_private_key", wrapped_private_key)?)?,
    )?;
    let ork = keys::open_sealed(&secret, &unb64("wrapped_ork", wrapped_ork)?)?;
    Ok(Some(key32("the organisation key", ork)?))
}

/// Open a proxy's credentials.
///
/// Two layers, and the reason is membership: the credentials are sealed under a
/// per-proxy data key, and only that key is wrapped under the organisation key.
/// Rotating the organisation key — which is what removing someone from the team
/// requires — then means re-wrapping one small key per proxy rather than
/// re-encrypting every secret the team owns.
pub fn open_proxy_credentials(
    org_key: &[u8; 32],
    credentials_enc: &[u8],
    wrapped_dek: &[u8],
) -> anyhow::Result<fury_shared::api::ProxyCredentials> {
    let dek = key32("the proxy data key", keys::unwrap(org_key, wrapped_dek)?)?;
    let plain = keys::unwrap(&dek, credentials_enc)?;
    Ok(serde_json::from_slice(&plain)?)
}

/// Seal a proxy's credentials for storage on the server.
#[allow(dead_code)] // Reached once the team-mode proxy form lands.
pub fn seal_proxy_credentials(
    org_key: &[u8; 32],
    credentials: &fury_shared::api::ProxyCredentials,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    // A fresh data key per proxy, so two proxies sharing a password do not
    // produce related ciphertext, and revoking one reveals nothing about
    // another.
    let dek = keys::new_org_key();
    let plain = serde_json::to_vec(credentials)?;
    Ok((keys::wrap(&dek, &plain), keys::wrap(org_key, &dek)))
}

/// A login sealed for the server: the blob, and the data key that opens it.
///
/// Same construction as a proxy's credentials and for the same reason. A fresh
/// data key per login means two accounts sharing a password do not produce
/// related ciphertext, and rotating the organisation key re-wraps a handful of
/// 32-byte keys instead of decrypting and re-encrypting every secret the team
/// owns.
pub fn seal_credential(
    org_key: &[u8; 32],
    payload: &serde_json::Value,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let dek = keys::new_org_key();
    let plain = serde_json::to_vec(payload)?;
    Ok((keys::wrap(&dek, &plain), keys::wrap(org_key, &dek)))
}

/// And back, on the machine of somebody who holds the organisation key.
pub fn open_credential(
    org_key: &[u8; 32],
    payload_enc_hex: &str,
    wrapped_dek_hex: &str,
) -> anyhow::Result<serde_json::Value> {
    let dek = keys::unwrap(org_key, &unhex("wrapped_dek", wrapped_dek_hex)?)?;
    let dek: [u8; 32] = dek
        .try_into()
        .map_err(|_| anyhow::anyhow!("the wrapped key is not 32 bytes"))?;
    let plain = keys::unwrap(&dek, &unhex("payload_enc", payload_enc_hex)?)?;
    Ok(serde_json::from_slice(&plain)?)
}

/// The key the agent is given for one profile's bundle.
///
/// Not the organisation key. See `fury_shared::keys::subkey`.
pub fn profile_key(org_key: &[u8; 32], profile_id: &str) -> [u8; 32] {
    keys::subkey(org_key, "fury-profile-v1", profile_id)
}

/// Move a data key from one organisation key to another.
///
/// The payload it protects is never touched. That is the whole reason the
/// credentials sit under a per-proxy data key rather than under the
/// organisation key directly: rotating means re-wrapping a handful of 32-byte
/// keys, not decrypting and re-encrypting every secret the team owns.
pub fn rewrap_data_key(old: &[u8; 32], new: &[u8; 32], wrapped_hex: &str) -> anyhow::Result<String> {
    let wrapped = unhex("wrapped_dek", wrapped_hex)?;
    let dek = keys::unwrap(old, &wrapped)?;
    Ok(hex(&keys::wrap(new, &dek)))
}

/// Move a bundle's data key from one profile key to another.
///
/// The stored form is the agent's (`agent/src/bundle.rs`): the shared prefix,
/// then base64. A bundle without the prefix is sealed under the machine that
/// packed it, has nothing to do with the organisation key, and is never handed
/// here -- the server lists only prefixed ones.
pub fn rewrap_bundle_key(old: &[u8; 32], new: &[u8; 32], stored: &str) -> anyhow::Result<String> {
    let rest = stored
        .strip_prefix(keys::SHARED_BUNDLE_PREFIX)
        .ok_or_else(|| anyhow::anyhow!("the bundle key is not sealed under a profile key"))?;
    let dek = keys::unwrap(old, &unb64("wrapped_dek", rest)?)?;
    Ok(format!("{}{}", keys::SHARED_BUNDLE_PREFIX, b64(&keys::wrap(new, &dek))))
}

/// The key a profile's cookies are encrypted with.
///
/// Stored, wrapped under the profile key, once the organisation key has been
/// rotated (migration 0020); derived from the profile key, as patch 0110 has
/// always done, before that. The two agree at the moment a rotation writes it
/// down -- the stored value IS the derived one, carried across -- so a profile
/// never sees its cookie key change.
pub fn cookie_key(
    profile_key: &[u8; 32],
    profile_id: &str,
    wrapped_hex: Option<&str>,
) -> anyhow::Result<[u8; 32]> {
    match wrapped_hex {
        Some(w) => key32("the cookie key", keys::unwrap(profile_key, &unhex("wrapped_cookie_key", w)?)?),
        None => Ok(keys::os_crypt_key(profile_key, profile_id)),
    }
}

/// A cookie key wrapped under a profile key, as the server stores it.
pub fn wrap_cookie_key(profile_key: &[u8; 32], cookie_key: &[u8; 32]) -> String {
    hex(&keys::wrap(profile_key, cookie_key))
}

/// Open a data key wrapped under the organisation key.
///
/// The first half of `rewrap_data_key`, needed on its own by sharing: the key
/// is not being moved to another organisation key, it is being sealed to one
/// person's public key.
pub fn unwrap_data_key(org_key: &[u8; 32], wrapped_hex: &str) -> anyhow::Result<[u8; 32]> {
    let wrapped = unhex("wrapped_dek", wrapped_hex)?;
    let dek = keys::unwrap(org_key, &wrapped)?;
    key32("the proxy data key", dek)
}

fn hex(v: &[u8]) -> String {
    v.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(what: &str, v: &str) -> anyhow::Result<Vec<u8>> {
    if v.len() % 2 != 0 || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
        anyhow::bail!("{what} is not hex");
    }
    (0..v.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&v[i..i + 2], 16).map_err(Into::into))
        .collect()
}

/// Hand the organisation key to somebody else.
///
/// Used when an admin admits a member who has enrolled and published a public
/// key. The result is stored by the server, which cannot open it.
pub fn wrap_for_member(org_key: &[u8; 32], member_public_key: &str) -> anyhow::Result<String> {
    let pk = key32("the member's public key", unb64("public_key", member_public_key)?)?;
    Ok(b64(&keys::seal_to(&pk, org_key)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a rotation does to one profile, end to end on the key side: the
    /// bundle key moves to the new profile key, and the cookie key the browser
    /// is handed afterwards is the one it had before.
    #[test]
    fn a_rotation_moves_the_bundle_key_and_keeps_the_cookie_key() {
        let (old_ork, new_ork) = (keys::new_org_key(), keys::new_org_key());
        let id = "01a010f3-69c8-7a72-bc3d-4d71698a5b4f";
        let (old_p, new_p) = (profile_key(&old_ork, id), profile_key(&new_ork, id));

        // As the agent seals it.
        let dek = keys::new_org_key();
        let stored = format!("{}{}", keys::SHARED_BUNDLE_PREFIX, b64(&keys::wrap(&old_p, &dek)));
        let moved = rewrap_bundle_key(&old_p, &new_p, &stored).unwrap();
        let opened = keys::unwrap(&new_p, &unb64("", moved.strip_prefix(keys::SHARED_BUNDLE_PREFIX).unwrap()).unwrap()).unwrap();
        assert_eq!(opened, dek);
        assert!(rewrap_bundle_key(&new_p, &old_p, &stored).is_err(), "opened under the wrong key");

        // Before any rotation nothing is stored and the key is derived.
        let before = cookie_key(&old_p, id, None).unwrap();
        let wrapped = wrap_cookie_key(&new_p, &before);
        assert_eq!(cookie_key(&new_p, id, Some(&wrapped)).unwrap(), before);
        // Derived from the new profile key it would differ -- the bug.
        assert_ne!(cookie_key(&new_p, id, None).unwrap(), before);

        // And a second rotation carries the stored one on again.
        let newer_p = profile_key(&keys::new_org_key(), id);
        let again = wrap_cookie_key(&newer_p, &cookie_key(&new_p, id, Some(&wrapped)).unwrap());
        assert_eq!(cookie_key(&newer_p, id, Some(&again)).unwrap(), before);
    }

    #[test]
    fn the_newcomer_and_the_owner_compute_the_same_verification_code() {
        let e = enrol("a password nobody else has", false).unwrap();
        // The newcomer: from the key opened with the password at a later
        // sign-in. The owner: from the key as enrolment published it.
        let opened = own_public_key("a password nobody else has", &e.kdf_salt, &e.wrapped_private_key).unwrap();
        assert_eq!(opened, e.public_key);
        let code = verify_code(&e.public_key).unwrap();
        assert_eq!(verify_code(&opened).unwrap(), code);
        assert!(code.len() == 9 && code.as_bytes()[4] == b'-' && code.replace('-', "").bytes().all(|b| b.is_ascii_digit()), "{code}");

        // Somebody else's key, as a stranger with the leaked code would have.
        let other = enrol("another password entirely", false).unwrap();
        assert_ne!(verify_code(&other.public_key).unwrap(), code);
        assert!(own_public_key("the wrong password", &e.kdf_salt, &e.wrapped_private_key).is_err());
    }

    #[test]
    fn an_owner_can_sign_in_again_and_get_the_same_organisation_key() {
        let e = enrol("a password nobody else has", true).unwrap();
        let recovered = unlock_org_key(
            "a password nobody else has",
            &e.kdf_salt,
            &e.wrapped_private_key,
            e.wrapped_ork.as_deref(),
        )
        .unwrap();
        assert_eq!(recovered, e.org_key);
        assert!(e.org_key.is_some());
    }

    #[test]
    fn the_wrong_password_recovers_nothing() {
        let e = enrol("the right one", true).unwrap();
        assert!(unlock_org_key(
            "the wrong one",
            &e.kdf_salt,
            &e.wrapped_private_key,
            e.wrapped_ork.as_deref()
        )
        .is_err());
    }

    #[test]
    fn a_member_enrols_without_an_organisation_key() {
        let e = enrol("a password nobody else has", false).unwrap();
        assert!(e.wrapped_ork.is_none());
        assert!(e.org_key.is_none());
        // And signing in is not an error — it is an account waiting to be let in.
        let got = unlock_org_key("a password nobody else has", &e.kdf_salt, &e.wrapped_private_key, None)
            .unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn an_admin_can_hand_the_key_to_a_member_who_then_reads_it() {
        let owner = enrol("owner password here", true).unwrap();
        let member = enrol("member password here", false).unwrap();

        let handed = wrap_for_member(&owner.org_key.unwrap(), &member.public_key).unwrap();
        let got = unlock_org_key(
            "member password here",
            &member.kdf_salt,
            &member.wrapped_private_key,
            Some(&handed),
        )
        .unwrap();

        assert_eq!(got, owner.org_key, "the member did not end up with the org's key");
    }

    #[test]
    fn proxy_credentials_survive_the_round_trip_and_travel_sealed() {
        let ork = fury_shared::keys::new_org_key();
        let creds = fury_shared::api::ProxyCredentials {
            username: Some("bob".into()),
            password: Some("s3cr3t-proxy-password".into()),
        };
        let (credentials_enc, wrapped_dek) = seal_proxy_credentials(&ork, &creds).unwrap();

        let back = open_proxy_credentials(&ork, &credentials_enc, &wrapped_dek).unwrap();
        assert_eq!(back.username.as_deref(), Some("bob"));
        assert_eq!(back.password.as_deref(), Some("s3cr3t-proxy-password"));

        // What the server stores must carry neither the password nor the key
        // that opens it.
        for blob in [&credentials_enc, &wrapped_dek] {
            assert!(!blob.windows(6).any(|w| w == b"s3cr3t"), "the password is in the clear");
            assert!(!blob.windows(32).any(|w| w == ork), "the organisation key travelled");
        }

        // Another organisation gets nothing.
        assert!(open_proxy_credentials(
            &fury_shared::keys::new_org_key(),
            &credentials_enc,
            &wrapped_dek
        )
        .is_err());
    }

    #[test]
    fn rotating_moves_a_data_key_without_touching_what_it_protects() {
        let old = fury_shared::keys::new_org_key();
        let new = fury_shared::keys::new_org_key();
        let creds = fury_shared::api::ProxyCredentials {
            username: Some("bob".into()),
            password: Some("s3cr3t".into()),
        };
        let (credentials_enc, wrapped_dek) = seal_proxy_credentials(&old, &creds).unwrap();

        let hex_dek = hex(&wrapped_dek);
        let rotated = unhex("x", &rewrap_data_key(&old, &new, &hex_dek).unwrap()).unwrap();

        // The credentials blob never moved, and it still opens — under the new
        // organisation key, and no longer under the old one.
        let back = open_proxy_credentials(&new, &credentials_enc, &rotated).unwrap();
        assert_eq!(back.password.as_deref(), Some("s3cr3t"));
        assert!(open_proxy_credentials(&old, &credentials_enc, &rotated).is_err());

        // And the member who left keeps a key that opens nothing new: the old
        // wrapping is still theirs, and it is no longer what the server holds.
        assert_ne!(rotated, wrapped_dek);
    }

    #[test]
    fn one_profiles_key_does_not_open_another() {
        let ork = fury_shared::keys::new_org_key();
        assert_ne!(profile_key(&ork, "profile-a"), profile_key(&ork, "profile-b"));
        assert_ne!(profile_key(&ork, "profile-a"), ork);
    }

    #[test]
    fn nothing_that_leaves_this_machine_contains_the_organisation_key() {
        let e = enrol("a password nobody else has", true).unwrap();
        let ork = e.org_key.unwrap();
        // What the client posts to the server, all of it.
        for (what, field) in [
            ("public_key", &e.public_key),
            ("wrapped_private_key", &e.wrapped_private_key),
            ("kdf_salt", &e.kdf_salt),
            ("wrapped_ork", e.wrapped_ork.as_ref().unwrap()),
        ] {
            let raw = unb64(what, field).unwrap();
            assert!(
                !raw.windows(32).any(|w| w == ork),
                "{what} carries the organisation key in the clear"
            );
        }
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    #[test]
    fn a_login_survives_the_server_and_opens_only_with_the_right_key() {
        let org = keys::new_org_key();
        let payload = serde_json::json!({
            "username": "alice",
            "password": "hunter2",
            "totp": "otpauth://totp/account?secret=JBSWY3DPEHPK3PXP&algorithm=SHA1&digits=6&period=30",
            "notes": "",
        });

        let (sealed, wrapped) = seal_credential(&org, &payload).unwrap();
        let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();

        // Nothing the server stores contains anything the server could read.
        // Checked on the bytes rather than trusted from the construction: this
        // is the one property the whole team feature rests on.
        let stored = hex(&sealed);
        for secret in ["hunter2", "alice", "JBSWY3DP"] {
            assert!(
                !String::from_utf8_lossy(&sealed).contains(secret),
                "{secret} is in the blob the server keeps"
            );
        }

        let back = open_credential(&org, &stored, &hex(&wrapped)).unwrap();
        assert_eq!(back, payload, "the round trip changed the login");

        // And the seed still produces the code it did before it travelled.
        // The two expected values were computed with an independent
        // implementation rather than by running this one and writing down what
        // it said, which would assert only that the code has not changed.
        let totp =
            fury_shared::totp::Totp::parse(back.get("totp").unwrap().as_str().unwrap()).unwrap();
        assert_eq!(totp.code_at(59), "996554");
        assert_eq!(totp.code_at(1234567890), "742275");

        // Another organisation's key opens nothing.
        let other = keys::new_org_key();
        assert!(
            open_credential(&other, &stored, &hex(&wrapped)).is_err(),
            "another organisation's key opened the login"
        );
    }

    #[test]
    fn two_logins_with_the_same_password_do_not_look_alike() {
        // Why there is a data key per login rather than sealing under the
        // organisation key directly. Two members of a team reusing a password
        // across accounts is normal, and identical ciphertext would tell the
        // server which accounts share one.
        let org = keys::new_org_key();
        let same = serde_json::json!({ "username": "a", "password": "p", "totp": null, "notes": "" });
        let (one, _) = seal_credential(&org, &same).unwrap();
        let (two, _) = seal_credential(&org, &same).unwrap();
        assert_ne!(one, two, "the same login sealed twice produced the same bytes");
    }
}

// ---- account recovery (docs/25, fury_shared::recovery) ----------------------

/// The private key, opened with the password, and refused unless it is the one
/// the server publishes for this account.
pub fn open_own_private_key(
    password: &str,
    kdf_salt: &str,
    wrapped_private_key: &str,
    public_key: &str,
) -> anyhow::Result<[u8; 32]> {
    let uk = keys::user_key(password, &unb64("kdf_salt", kdf_salt)?)?;
    let secret = key32(
        "the wrapped private key",
        keys::unwrap(&uk, &unb64("wrapped_private_key", wrapped_private_key)?)?,
    )?;
    if keys::public_from_secret(&secret).to_vec() != unb64("public_key", public_key)? {
        anyhow::bail!("the private key does not belong to this account");
    }
    Ok(secret)
}

/// A new recovery code for `secret`: the printed code, shown once, and what the
/// server keeps (`salt`, `wrapped_key`, `auth`, base64).
pub fn new_recovery(secret: &[u8; 32]) -> anyhow::Result<(String, serde_json::Value)> {
    let (raw, printed) = fury_shared::recovery::new_code();
    let m = fury_shared::recovery::seal(&raw, secret)?;
    Ok((
        printed,
        serde_json::json!({ "salt": b64(&m.salt), "wrapped_key": b64(&m.wrapped_private_key), "auth": b64(&m.auth) }),
    ))
}

/// The private key, opened with a recovery code and checked against the
/// published public key.
pub fn open_with_recovery(
    raw: &[u8; 32],
    salt: &str,
    wrapped_key: &str,
    public_key: &str,
) -> anyhow::Result<[u8; 32]> {
    let d = fury_shared::recovery::derive(raw, &unb64("salt", salt)?)?;
    let public = key32("the public key", unb64("public_key", public_key)?)?;
    Ok(fury_shared::recovery::open(&d, &unb64("wrapped_key", wrapped_key)?, &public)?)
}

/// The server's challenge, opened: the proof that this client holds the key.
pub fn answer_challenge(secret: &[u8; 32], challenge: &str) -> anyhow::Result<String> {
    Ok(b64(&keys::open_sealed(secret, &unb64("challenge", challenge)?)?))
}

/// `secret` wrapped under a new password: a fresh salt and the wrapping.
pub fn rewrap_for_password(password: &str, secret: &[u8; 32]) -> anyhow::Result<(String, String)> {
    let salt = keys::new_salt();
    let uk = keys::user_key(password, &salt)?;
    Ok((b64(&salt), b64(&keys::wrap(&uk, secret))))
}

