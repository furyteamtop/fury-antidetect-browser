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
