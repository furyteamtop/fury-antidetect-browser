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

/// Hand the organisation key to somebody else.
///
/// Used when an admin admits a member who has enrolled and published a public
/// key. The result is stored by the server, which cannot open it.
#[allow(dead_code)] // Reached once the member-approval screen lands.
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
