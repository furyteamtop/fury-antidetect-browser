// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Invitations — the only way an account comes into existence.
//!
//! # Why there is no registration form
//!
//! A Fury server holds the working accounts of a team. Open registration on
//! such a server is a door without a lock. So an account starts as an
//! invitation issued by someone who already runs the box, or already runs the
//! organisation.
//!
//! # Why the server cannot simply create the user
//!
//! Because it cannot produce what a user is made of. A user has an X25519
//! keypair whose private half is wrapped under a key derived from their
//! password, and a copy of the organisation key wrapped to that keypair. All of
//! it is generated on the client, from a password the server is never sent in a
//! form it could derive anything from. The server issues a code and, later,
//! accepts the wrapped results. It stores material it can do nothing with,
//! which is the entire point of the design in docs/04.
//!
//! # The bootstrap
//!
//! The first person on a fresh server has nobody to invite them, so the code is
//! minted at the command line by whoever has a shell on the machine:
//!
//! ```text
//! fury-server invite --email you@example.com --org "My team"
//! ```
//!
//! That produces an owner invitation, which carries an organisation name rather
//! than an id: the org is created at the moment it acquires an owner, so a
//! server whose bootstrap was abandoned halfway is left with nothing rather
//! than an org nobody can enter.

use uuid::Uuid;

/// How long an invitation lives.
///
/// Long enough to walk to another desk or send a message; short enough that a
/// code forgotten in a chat log is not a standing way in. Re-issuing costs one
/// command.
pub const TTL_HOURS: i64 = 48;

/// A code, as shown to a person.
///
/// 160 bits from the OS CSPRNG, printed in lowercase base32 without padding and
/// grouped in fours. Base32 rather than hex because these get dictated over a
/// call, and the alphabet drops the characters that get misheard.
pub fn new_code() -> (String, Vec<u8>) {
    use rand::RngCore;
    let mut bytes = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut bytes);

    // Crockford's alphabet: no I, L, O or U, so nothing reads as a 1, a 0, or
    // an unfortunate word.
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut out = String::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for b in bytes {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let idx = ((acc >> bits) & 0x1f) as usize;
            if out.len() % 5 == 4 && !out.is_empty() {
                out.push('-');
            }
            out.push(ALPHABET[idx] as char);
        }
    }
    let digest = hash_code(&out);
    (out, digest)
}

/// The stored form of a code.
///
/// SHA-256, not a password KDF: the input is 160 bits of uniform randomness,
/// so there is no dictionary to attack and a slow hash buys nothing. Separators
/// and case are normalised away first, because a person retyping a code from a
/// screen will not reproduce the hyphens exactly.
pub fn hash_code(raw: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let normalised: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    Sha256::digest(normalised.as_bytes()).to_vec()
}

/// Mint an invitation. Returns the code, which is shown once and never stored.
pub async fn issue(
    // A connection rather than the pool, because the caller has one bound to
    // the request's user and the invitation is written beside audit rows that
    // now require it. `enrollments` itself carries no policy — an invitation
    // is peeked at by somebody who is not yet anybody — but taking the pool
    // here would quietly step outside the caller's identity for one statement.
    db: &mut sqlx::PgConnection,
    email: &str,
    org: Org<'_>,
    role: &str,
) -> anyhow::Result<String> {
    let (code, digest) = new_code();
    let (org_id, org_name) = match org {
        Org::Existing(id) => (Some(id), None),
        Org::New(name) => (None, Some(name)),
    };

    sqlx::query(
        "INSERT INTO enrollments (id, code_hash, email, org_id, org_name, role, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6::org_role, now() + ($7 || ' hours')::interval)",
    )
    .bind(Uuid::now_v7())
    .bind(&digest)
    .bind(email)
    .bind(org_id)
    .bind(org_name)
    .bind(role)
    .bind(TTL_HOURS.to_string())
    .execute(&mut *db)
    .await?;

    Ok(code)
}

pub enum Org<'a> {
    Existing(Uuid),
    New(&'a str),
}

/// `fury-server invite --email … [--org "Name" | --org-id UUID] [--role member]`
///
/// Lives in the server binary rather than a separate tool because it needs the
/// database URL the server already knows how to find, and because a self-hoster
/// who has the server has the command.
pub async fn cli(args: &[String]) -> anyhow::Result<()> {
    let mut email = None;
    let mut org_name = None;
    let mut org_id = None;
    let mut role = "owner".to_string();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--email" => email = it.next().cloned(),
            "--org" => org_name = it.next().cloned(),
            "--org-id" => org_id = it.next().map(|v| v.parse()).transpose()?,
            "--role" => {
                if let Some(v) = it.next() {
                    role = v.clone();
                }
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }

    let email = email.ok_or_else(|| anyhow::anyhow!("--email is required"))?;
    if !matches!(role.as_str(), "owner" | "admin" | "manager" | "member") {
        anyhow::bail!("--role must be one of: owner, admin, manager, member");
    }

    let db = crate::connect().await?;
    sqlx::migrate!("./migrations").run(&db).await?;

    let org = match (org_id, org_name.as_deref()) {
        (Some(id), None) => Org::Existing(id),
        (None, Some(name)) => Org::New(name),
        (None, None) => anyhow::bail!(
            "give either --org \"Name\" to create an organisation, or --org-id to join one"
        ),
        (Some(_), Some(_)) => anyhow::bail!("--org and --org-id are mutually exclusive"),
    };

    // An owner is what a new organisation gets; joining an existing one as
    // owner would make a second, which the ORK design has no story for.
    if matches!(org, Org::Existing(_)) && role == "owner" {
        anyhow::bail!("an existing organisation already has an owner — invite as admin instead");
    }

    // The `invite` command runs as the operator on the machine, not as a
    // request, so there is no bound connection to take — and none is needed:
    // an invitation names an organisation the operator chose at the terminal.
    let mut conn = db.acquire().await?;
    let code = issue(&mut conn, &email, org, &role).await?;

    println!("Invitation for {email} ({role}), valid {TTL_HOURS} hours:\n");
    println!("    {code}\n");
    println!("Enter it in Fury under \"I have an invitation\", together with this");
    println!("server's address. The code is shown once — it is not stored.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_readable_and_unambiguous() {
        let (code, _) = new_code();
        // 160 bits at 5 bits a character, hyphenated in groups of four.
        assert_eq!(code.chars().filter(|c| *c != '-').count(), 32);
        assert!(code.split('-').all(|g| g.len() == 4), "uneven grouping: {code}");
        assert!(
            !code.contains(['i', 'l', 'o', 'u']),
            "the alphabet must not contain characters that are misheard: {code}"
        );
    }

    #[test]
    fn two_codes_differ() {
        assert_ne!(new_code().0, new_code().0);
    }

    #[test]
    fn a_code_matches_however_it_was_retyped() {
        let (code, digest) = new_code();
        // What someone types back from a screen: no hyphens, or upper case, or
        // spaces where the hyphens were. All of it is the same code.
        assert_eq!(hash_code(&code.replace('-', "")), digest);
        assert_eq!(hash_code(&code.to_uppercase()), digest);
        assert_eq!(hash_code(&code.replace('-', " ")), digest);
    }

    #[test]
    fn a_different_code_does_not_match() {
        let (_, digest) = new_code();
        assert_ne!(new_code().1, digest);
    }
}
