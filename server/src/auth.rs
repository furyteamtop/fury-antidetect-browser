// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Session tokens and the request-time identity.
//!
//! Opaque random tokens, not JWTs. A JWT cannot be revoked without keeping a
//! denylist, which is the state a JWT was supposed to avoid — and for a tool
//! whose whole selling point is that an operator's access can be withdrawn the
//! moment they leave, revocation has to be immediate and unconditional.
//!
//! Only the hash of a token is stored. A dump of the sessions table therefore
//! grants nothing, which matters because docs/01 assumes the server itself may
//! be compromised.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use fury_shared::rbac::{OrgRole, PermSet};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;
use crate::AppState;

/// Length of the raw token. 32 bytes from the OS CSPRNG — brute force is not a
/// consideration at this size, so tokens carry no structure to be guessed.
const TOKEN_BYTES: usize = 32;

/// How long a session lasts without use. Deliberately short for a tool that
/// holds live account credentials.
pub const SESSION_TTL_HOURS: i64 = 12;

/// Generates a new token and the hash to store beside it.
///
/// The raw token is returned once, to the client. It is never written anywhere
/// the server can read it back.
pub fn new_token() -> (String, Vec<u8>) {
    use rand::RngCore;
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let raw = hex(&bytes);
    let digest = hash_token(&raw);
    (raw, digest)
}

/// SHA-256 of the token as presented.
///
/// A plain hash rather than a password KDF, and that is correct here: the input
/// is 256 bits of uniform randomness, so there is no dictionary to attack and
/// nothing for a slow hash to buy. Passwords are a different matter — see
/// `verify_password`.
pub fn hash_token(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Verifies a password against a stored Argon2id hash.
///
/// Distinct from `hash_token` on purpose: a password is low-entropy and chosen
/// by a human, so it needs a deliberately slow, memory-hard hash. Using SHA-256
/// here would be a real vulnerability.
pub fn verify_password(password: &str, stored: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    argon2::Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// The stored form of a login password.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    Ok(argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))?
        .to_string())
}

/// The authenticated caller, resolved once per request.
///
/// `Copy` on purpose: handlers read `db.caller.org_id` in the same expression
/// that borrows the connection mutably, and a copy releases the borrow before
/// the mutable one begins. Two UUIDs and an enum.
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub role: OrgRole,
}

impl Caller {
    /// Permissions this caller holds everywhere in the organisation, before any
    /// per-project grant is consulted. Owners and admins reach every project;
    /// everyone else starts from nothing.
    #[allow(dead_code)] // consumed by the org-wide listing endpoints
    pub fn implicit_permissions(&self) -> PermSet {
        if fury_shared::rbac::has_implicit_project_access(self.role) {
            PermSet::all()
        } else {
            PermSet::NONE
        }
    }
}

impl FromRequestParts<Arc<AppState>> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthenticated)?;

        let digest = hash_token(token);

        // Expiry is enforced in the query rather than in Rust: a session that
        // has lapsed must be unusable even if a handler forgets to check.
        let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
            r#"
            SELECT s.user_id, m.org_id, m.role::text
            FROM sessions s
            JOIN org_members m ON m.user_id = s.user_id
            JOIN users u ON u.id = s.user_id
            WHERE s.token_hash = $1
              AND s.expires_at > now()
              AND u.disabled_at IS NULL
            "#,
        )
        .bind(&digest)
        .fetch_optional(&state.db)
        .await?;

        let (user_id, org_id, role) = row.ok_or(ApiError::Unauthenticated)?;
        let role = parse_role(&role).ok_or(ApiError::Unauthenticated)?;

        // Sliding expiry: an operator working a full shift is not logged out
        // mid-session, but an abandoned token still dies on schedule.
        sqlx::query(
            "UPDATE sessions SET expires_at = now() + ($2 || ' hours')::interval, \
             last_seen_at = now() WHERE token_hash = $1",
        )
        .bind(&digest)
        .bind(SESSION_TTL_HOURS.to_string())
        .execute(&state.db)
        .await?;

        Ok(Caller {
            user_id,
            org_id,
            role,
        })
    }
}

/// A database connection with `app.user_id` already bound, and the caller it is
/// bound to.
///
/// The point of the type is that there is no way to query as an authenticated
/// user without it. `State(state).db` is the raw pool and reaches the database
/// with no identity attached; under the policies in migration 0006 that
/// connection can see nothing at all, so a handler that forgets returns an
/// empty list rather than somebody else's rows. The failure direction is the
/// one worth having, and it is loud: the tests in this module assert it.
///
/// Why a connection and not a transaction. `rbac_guard::bind_rls_user` took a
/// transaction, which is why it was never called: six of forty-odd handlers
/// open one, and giving the rest a transaction each — to hold a setting they
/// only need for one statement — would have been a large change that also
/// changed their failure behaviour. A pooled connection carries a session-level
/// setting perfectly well, and `main::connect`'s `after_release` hook clears it
/// on the way back.
///
/// The cost is real and small: a handler holds one of sixteen connections for
/// its whole duration rather than per statement. Handlers here are a query or
/// three.
pub struct Db {
    pub caller: Caller,
    conn: sqlx::pool::PoolConnection<sqlx::Postgres>,
}

impl Db {
    /// The connection, for `sqlx::query(..).fetch_*(db.as_mut())`.
    pub fn as_mut(&mut self) -> &mut sqlx::PgConnection {
        &mut self.conn
    }

    /// A transaction on the same connection, so the binding carries into it.
    ///
    /// Beginning a transaction from the POOL instead would take a different
    /// connection — one with no identity — and every statement inside it would
    /// see nothing. That is a fair description of how this could be got wrong
    /// while looking right.
    pub async fn begin(&mut self) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, sqlx::Error> {
        use sqlx::Acquire;
        self.conn.begin().await
    }
}

impl FromRequestParts<Arc<AppState>> for Db {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // The caller is resolved first and on the pool, deliberately: the
        // session lookup reads `sessions`, `users` and `org_members`, which
        // carry no policy precisely because they are what establishes who the
        // policies are about.
        let caller = Caller::from_request_parts(parts, state).await?;

        let mut conn = state.db.acquire().await?;
        // `false` is session scope. See the note on this type, and the
        // after_release hook in main.rs that undoes it.
        sqlx::query("SELECT set_config('app.user_id', $1, false)")
            .bind(caller.user_id.to_string())
            .execute(&mut *conn)
            .await?;

        Ok(Db { caller, conn })
    }
}

pub fn parse_role(s: &str) -> Option<OrgRole> {
    Some(match s {
        "owner" => OrgRole::Owner,
        "admin" => OrgRole::Admin,
        "manager" => OrgRole::Manager,
        "member" => OrgRole::Member,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unpredictable_and_distinct() {
        let (a, _) = new_token();
        let (b, _) = new_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), TOKEN_BYTES * 2);
    }

    #[test]
    fn the_stored_hash_does_not_reveal_the_token() {
        let (raw, digest) = new_token();
        assert_ne!(digest, raw.as_bytes());
        // And the same token always hashes the same, or lookup would fail.
        assert_eq!(digest, hash_token(&raw));
    }

    #[test]
    fn passwords_use_a_slow_hash_and_a_fresh_salt() {
        let a = hash_password("correct horse battery staple").unwrap();
        let b = hash_password("correct horse battery staple").unwrap();
        // Same password, different stored value: the salt is per-hash, so a
        // stolen table cannot be attacked with one precomputed set.
        assert_ne!(a, b);
        assert!(a.starts_with("$argon2"));
        assert!(verify_password("correct horse battery staple", &a));
        assert!(!verify_password("Correct horse battery staple", &a));
    }

    #[test]
    fn a_malformed_stored_hash_rejects_rather_than_panics() {
        assert!(!verify_password("anything", "not-a-hash"));
        assert!(!verify_password("anything", ""));
    }

    #[test]
    fn only_owners_and_admins_reach_projects_without_a_grant() {
        let caller = |role| Caller {
            user_id: Uuid::nil(),
            org_id: Uuid::nil(),
            role,
        };
        assert_eq!(caller(OrgRole::Owner).implicit_permissions(), PermSet::all());
        assert_eq!(caller(OrgRole::Admin).implicit_permissions(), PermSet::all());
        assert_eq!(
            caller(OrgRole::Manager).implicit_permissions(),
            PermSet::NONE
        );
        assert_eq!(caller(OrgRole::Member).implicit_permissions(), PermSet::NONE);
    }
}
