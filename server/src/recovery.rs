// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Account recovery and owner hand-over (docs/25).
//!
//! The server cannot reset a password. The private key is wrapped under a key
//! the password becomes on the client, and a new password does not open the
//! old wrapping: "reset" would leave the account unable to read anything its
//! team ever encrypted. What it can do is keep a second wrapping, under a key
//! made from a recovery code only the person holds, and let them swap the
//! password once they show the code.
//!
//! The order of a recovery, and who proves what:
//!
//! 1. `start`: the client derives the code's "auth" half and sends it. The
//!    server checks it against an Argon2id hash. Five misses an hour per
//!    address close the door for the hour; an unknown address, an account
//!    with no key and a wrong code are one answer.
//! 2. The second step, fixed at start: a TOTP code if the account has one,
//!    else an emailed code if this server sends mail, else a 24-hour wait that
//!    the team's owner and admins, and the account's own live sessions, see
//!    and can cancel (`pending`, `cancel`).
//! 3. `material`: the wrapped key, its salt, the published public key, and a
//!    nonce sealed to that key.
//! 4. `finish`: the opened nonce (proof of the private key), the new password
//!    and its wrapping, and a NEW recovery key: the spent one stops working in
//!    the same transaction. Every session of the account is revoked and a new
//!    one issued.
//!
//! The organisation key is untouched throughout: it is sealed to the same
//! public key, which recovery keeps.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::{self, Db};
use crate::error::{ApiError, ApiResult};
use crate::security::{self, Attempt};
use crate::AppState;

/// How long the wait lasts when there is no second factor to ask for.
pub const WAIT_HOURS: i64 = 24;
/// How long an attempt stays open: long enough to come back after the wait.
const LIFETIME_HOURS: i64 = 72;
/// An attempt that has passed its factor and not finished in this long lapses.
const EMAIL_CODE_MINUTES: i64 = 15;
/// Wrong second-step codes before the attempt ends.
const MAX_CODE_ATTEMPTS: i32 = 5;
/// Wrong recovery keys per address per hour.
const MAX_KEY_FAILURES: i64 = 5;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

fn decode(field: &str, v: &str) -> ApiResult<Vec<u8>> {
    B64.decode(v.trim()).map_err(|_| ApiError::BadRequest(format!("{field} is not base64")))
}

fn sized(field: &str, v: &str, len: usize) -> ApiResult<Vec<u8>> {
    let b = decode(field, v)?;
    if b.len() != len {
        return Err(ApiError::BadRequest(format!("{field} must be {len} bytes")));
    }
    Ok(b)
}

/// A wrapped 32-byte key: 24-byte nonce, 32 bytes, 16-byte tag.
const WRAPPED_KEY_LEN: usize = 24 + 32 + 16;

/// What a client sends to set a recovery key, now or at the end of a recovery.
#[derive(Deserialize)]
pub struct KeyMaterial {
    pub salt: String,
    pub wrapped_key: String,
    /// The code's "auth" half, base64. Hashed here and dropped.
    pub auth: String,
}

struct Checked {
    salt: Vec<u8>,
    wrapped: Vec<u8>,
    auth_hash: String,
}

fn check_material(m: &KeyMaterial) -> ApiResult<Checked> {
    let salt = sized("salt", &m.salt, fury_shared::keys::SALT_LEN)?;
    let wrapped = sized("wrapped_key", &m.wrapped_key, WRAPPED_KEY_LEN)?;
    let auth = sized("auth", &m.auth, fury_shared::keys::KEY_LEN)?;
    let auth_hash = auth::hash_password(&B64.encode(auth)).map_err(|e| ApiError::Internal(anyhow::anyhow!(e.to_string())))?;
    Ok(Checked { salt, wrapped, auth_hash })
}

// ---------------------------------------------------------------------------
// signed in: making a key, seeing and cancelling attempts
// ---------------------------------------------------------------------------

/// `GET /v1/me/recovery`
///
/// With the account's wrapped private key, its salt and its public key: what a
/// client needs, with the password, to open the private key and lock it under a
/// new recovery code. The same three a sign-in already hands over.
pub async fn status(mut db: Db) -> ApiResult<Json<serde_json::Value>> {
    let me = db.caller.user_id;
    let (created, kdf_salt, wrapped, public): (Option<String>, Vec<u8>, Vec<u8>, Vec<u8>) = sqlx::query_as(&format!(
        "SELECT {}, kdf_salt, wrapped_private_key, public_key FROM users WHERE id = $1",
        crate::api::rfc3339("recovery_created_at")
    ))
    .bind(me)
    .fetch_one(db.as_mut())
    .await?;
    let pending = open_attempts(db.as_mut(), "r.user_id = $1", me).await?;
    Ok(Json(json!({
        "has_key": created.is_some(),
        "created_at": created,
        "pending": pending,
        "kdf_salt": B64.encode(kdf_salt),
        "wrapped_private_key": B64.encode(wrapped),
        "public_key": B64.encode(public),
    })))
}

#[derive(Deserialize)]
pub struct SetRequest {
    pub password: String,
    #[serde(default)]
    pub totp_code: Option<String>,
    #[serde(flatten)]
    pub material: KeyMaterial,
}

/// `POST /v1/me/recovery` -- make or replace this account's recovery key.
///
/// The password again, and the TOTP code when the account has one: a key
/// that can take the account over is not something a person walking past an
/// unlocked laptop should be able to mint.
pub async fn set(mut db: Db, Json(req): Json<SetRequest>) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    reauthenticate(db.as_mut(), caller.user_id, &req.password, req.totp_code.as_deref()).await?;
    let m = check_material(&req.material)?;
    sqlx::query(
        "UPDATE users SET recovery_salt = $2, recovery_wrapped_key = $3, recovery_auth_hash = $4, \
                recovery_created_at = now() WHERE id = $1",
    )
    .bind(caller.user_id)
    .bind(&m.salt)
    .bind(&m.wrapped)
    .bind(&m.auth_hash)
    .execute(db.as_mut())
    .await?;
    crate::api::audit(db.as_mut(), &caller, "account.recovery_key_set", None, json!({})).await?;
    Ok(Json(json!({ "has_key": true })))
}

/// Password, then the TOTP code if the account has one. One answer for both
/// misses, as at sign-in.
async fn reauthenticate(
    db: &mut sqlx::PgConnection,
    user: Uuid,
    password: &str,
    totp_code: Option<&str>,
) -> ApiResult<()> {
    let (hash, secret, enrolled): (String, Option<Vec<u8>>, bool) = sqlx::query_as(
        "SELECT password_hash, totp_secret, totp_enabled_at IS NOT NULL FROM users WHERE id = $1",
    )
    .bind(user)
    .fetch_one(&mut *db)
    .await?;
    if !auth::verify_password(password, &hash) {
        return Err(ApiError::Refused("wrong_password"));
    }
    if enrolled {
        match (secret, totp_code) {
            (Some(s), Some(code)) if security::code_matches(&s, code) => {}
            (_, None) => return Err(ApiError::Refused("totp_required")),
            _ => return Err(ApiError::Refused("wrong_code")),
        }
    }
    Ok(())
}

/// Attempts not finished, cancelled or lapsed, matching `filter` on `$1`.
async fn open_attempts(
    db: &mut sqlx::PgConnection,
    filter: &str,
    bind: Uuid,
) -> ApiResult<Vec<serde_json::Value>> {
    let rows: Vec<(Uuid, String, String, String, Option<String>, Option<String>, String)> = sqlx::query_as(&format!(
        "SELECT r.id, u.email::text, r.factor, {}, {}, host(r.ip), r.machine_name \
         FROM recovery_requests r JOIN users u ON u.id = r.user_id \
         WHERE {filter} AND r.cancelled_at IS NULL AND r.completed_at IS NULL AND r.expires_at > now() \
         ORDER BY r.created_at DESC",
        crate::api::rfc3339("r.created_at"),
        crate::api::rfc3339("r.ready_at"),
    ))
    .bind(bind)
    .fetch_all(&mut *db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, email, factor, created_at, ready_at, ip, machine_name)| {
            json!({ "id": id, "email": email, "factor": factor, "created_at": created_at,
                    "ready_at": ready_at, "ip": ip, "machine_name": machine_name })
        })
        .collect())
}

/// `GET /v1/org/recovery` -- the team's open attempts, for the owner and
/// admins: the people a 24-hour wait exists to warn.
pub async fn org_pending(mut db: Db) -> ApiResult<Json<serde_json::Value>> {
    use fury_shared::rbac::OrgRole;
    let caller = db.caller;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Ok(Json(json!([])));
    }
    let rows = open_attempts(
        db.as_mut(),
        "r.user_id IN (SELECT user_id FROM org_members WHERE org_id = $1)",
        caller.org_id,
    )
    .await?;
    Ok(Json(json!(rows)))
}

/// `POST /v1/recovery/{id}/cancel` -- by the account itself from a live
/// session, or by the owner or an admin of its team.
pub async fn cancel(mut db: Db, Path(id): Path<Uuid>) -> ApiResult<Json<serde_json::Value>> {
    use fury_shared::rbac::OrgRole;
    let caller = db.caller;
    let staff = matches!(caller.role, OrgRole::Owner | OrgRole::Admin);
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT r.user_id FROM recovery_requests r \
         WHERE r.id = $1 AND r.cancelled_at IS NULL AND r.completed_at IS NULL \
           AND (r.user_id = $2 OR ($3 AND r.user_id IN (SELECT user_id FROM org_members WHERE org_id = $4)))",
    )
    .bind(id)
    .bind(caller.user_id)
    .bind(staff)
    .bind(caller.org_id)
    .fetch_optional(db.as_mut())
    .await?;
    let Some((target,)) = row else { return Err(ApiError::NotFound) };
    sqlx::query("UPDATE recovery_requests SET cancelled_at = now(), cancelled_by = $2 WHERE id = $1")
        .bind(id)
        .bind(caller.user_id)
        .execute(db.as_mut())
        .await?;
    crate::api::audit(db.as_mut(), &caller, "account.recovery_cancelled", Some(target), json!({ "request": id })).await?;
    Ok(Json(json!({ "cancelled": true })))
}

// ---------------------------------------------------------------------------
// not signed in: the recovery itself
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct StartRequest {
    pub email: String,
    /// The code's "auth" half, base64.
    pub auth: String,
    #[serde(default)]
    pub machine_name: String,
    #[serde(default)]
    pub machine_id: Option<String>,
    /// For the letter, when the second step is an emailed code.
    #[serde(default)]
    pub lang: String,
}

/// `POST /v1/auth/recover/start`
pub async fn start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<StartRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let ip = security::client_ip(&headers);
    let ua = security::user_agent(&headers);
    let email = req.email.trim().to_string();
    let attempt = |org, user, outcome: &'static str| Attempt {
        org, user, email: &email, outcome, ip,
        machine_name: &req.machine_name, machine_id: req.machine_id.as_deref(), user_agent: ua.as_deref(),
    };

    let (failures,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM recovery_failures WHERE email = $1 AND at > now() - interval '1 hour'",
    )
    .bind(&email)
    .fetch_one(&state.db)
    .await?;
    if failures >= MAX_KEY_FAILURES {
        return Err(ApiError::Refused("too_many_attempts"));
    }

    let row: Option<(Uuid, String, Option<String>, bool)> = sqlx::query_as(
        "SELECT id, email::text, recovery_auth_hash, totp_enabled_at IS NOT NULL \
         FROM users WHERE email = $1 AND disabled_at IS NULL",
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;
    let candidate = B64.encode(decode("auth", &req.auth).unwrap_or_default());
    let found = match &row {
        Some((id, mail, Some(hash), totp)) if auth::verify_password(&candidate, hash) => Some((*id, mail.clone(), *totp)),
        Some((_, _, Some(_), _)) => None,
        _ => {
            // The time a verification takes, so "no key" and "no account" do
            // not answer faster than "wrong key".
            let _ = auth::verify_password(&candidate, "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$0000000000000000000000000000000000000000000");
            None
        }
    };
    let org = match &row {
        Some((id, ..)) => security::org_of(&state.db, *id).await,
        None => None,
    };
    let Some((user_id, account_email, totp)) = found else {
        sqlx::query("DELETE FROM recovery_failures WHERE at < now() - interval '1 day'").execute(&state.db).await?;
        sqlx::query("INSERT INTO recovery_failures (email, ip) VALUES ($1, $2::inet)")
            .bind(&email)
            .bind(ip.map(|i| i.to_string()))
            .execute(&state.db)
            .await?;
        security::record_login(&state.db, attempt(org, row.as_ref().map(|r| r.0), "recovery_bad")).await;
        return Err(ApiError::Unauthenticated);
    };

    // The organisation's address list applies here as at sign-in.
    if let Some(o) = org {
        let role: Option<(String,)> = sqlx::query_as("SELECT role::text FROM org_members WHERE user_id = $1 AND org_id = $2")
            .bind(user_id)
            .bind(o)
            .fetch_optional(&state.db)
            .await?;
        let role = role.and_then(|(r,)| auth::parse_role(&r)).unwrap_or(fury_shared::rbac::OrgRole::Member);
        let mut conn = state.db.acquire().await?;
        let policy = security::policy_of(&mut conn, o).await?;
        if !policy.admits(ip, role) {
            security::record_login(&state.db, attempt(org, Some(user_id), "ip_refused")).await;
            return Err(ApiError::Refused("ip_not_allowed"));
        }
    }

    let factor = if totp {
        "totp"
    } else if state.mailer.is_some() {
        "email"
    } else {
        "wait"
    };

    // One attempt at a time: a new one ends the old.
    sqlx::query(
        "UPDATE recovery_requests SET cancelled_at = now() \
         WHERE user_id = $1 AND cancelled_at IS NULL AND completed_at IS NULL",
    )
    .bind(user_id)
    .execute(&state.db)
    .await?;

    let id = Uuid::now_v7();
    let (secret, secret_hash) = auth::new_token();
    let digits = (factor == "email").then(|| {
        use rand::Rng;
        format!("{:06}", rand::rngs::OsRng.gen_range(0..1_000_000u32))
    });
    sqlx::query(
        "INSERT INTO recovery_requests \
           (id, user_id, secret_hash, factor, code_hash, ready_at, ip, machine_name, machine_id, user_agent, expires_at) \
         VALUES ($1, $2, $3, $4, $5, \
                 CASE WHEN $4 = 'wait' THEN now() + ($6 || ' hours')::interval END, \
                 $7::inet, $8, $9, $10, now() + ($11 || ' hours')::interval)",
    )
    .bind(id)
    .bind(user_id)
    .bind(&secret_hash)
    .bind(factor)
    .bind(digits.as_deref().map(|d| code_hash(id, d)))
    .bind(WAIT_HOURS.to_string())
    .bind(ip.map(|i| i.to_string()))
    .bind(&req.machine_name)
    .bind(req.machine_id.as_deref())
    .bind(ua.as_deref())
    .bind(LIFETIME_HOURS.to_string())
    .execute(&state.db)
    .await?;

    if let (Some(d), Some(mailer)) = (&digits, &state.mailer) {
        let (subject, body) = crate::mail::recovery_code_letter(&req.lang, d, EMAIL_CODE_MINUTES);
        if let Err(e) = mailer.send(&account_email, &subject, body).await {
            tracing::warn!(error = %e, "could not send a recovery code");
            sqlx::query("UPDATE recovery_requests SET cancelled_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
            return Err(ApiError::BadRequest("the letter with the code could not be sent; try again in a minute".into()));
        }
    }

    security::record_login(&state.db, attempt(org, Some(user_id), "recovery_started")).await;
    if let Some(o) = org {
        security::audit_login_event(&state.db, o, user_id, "account.recovery_started", json!({
            "factor": factor, "ip": ip.map(|i| i.to_string()), "machine_name": req.machine_name,
        })).await;
    }
    let ready_at: Option<(Option<String>,)> = sqlx::query_as(&format!(
        "SELECT {} FROM recovery_requests WHERE id = $1",
        crate::api::rfc3339("ready_at")
    ))
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    Ok(Json(json!({
        "request": id,
        "secret": secret,
        "factor": factor,
        "ready_at": ready_at.and_then(|(r,)| r),
    })))
}

fn code_hash(request: Uuid, digits: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    Sha256::digest(format!("{request}:{digits}").as_bytes()).to_vec()
}

#[derive(Deserialize)]
pub struct Handle {
    pub request: Uuid,
    pub secret: String,
}

struct Open {
    user_id: Uuid,
    factor: String,
    code_hash: Option<Vec<u8>>,
    attempts: i32,
    ready: bool,
    ready_at: Option<String>,
    challenge_hash: Option<Vec<u8>>,
}

/// The attempt a handle names, if it is still open. Anything else -- a wrong
/// secret, cancelled, finished, lapsed -- is one answer.
async fn open_attempt(db: &sqlx::PgPool, h: &Handle) -> ApiResult<Open> {
    let row: Option<(Uuid, String, Option<Vec<u8>>, i32, bool, Option<String>, Option<Vec<u8>>)> = sqlx::query_as(&format!(
        "SELECT user_id, factor, code_hash, attempts, coalesce(ready_at <= now(), false), {}, challenge_hash \
         FROM recovery_requests \
         WHERE id = $1 AND secret_hash = $2 AND cancelled_at IS NULL AND completed_at IS NULL AND expires_at > now()",
        crate::api::rfc3339("ready_at")
    ))
    .bind(h.request)
    .bind(auth::hash_token(&h.secret))
    .fetch_optional(db)
    .await?;
    let (user_id, factor, code_hash, attempts, ready, ready_at, challenge_hash) = row.ok_or(ApiError::Refused("recovery_closed"))?;
    Ok(Open { user_id, factor, code_hash, attempts, ready, ready_at, challenge_hash })
}

/// `POST /v1/auth/recover/status` -- where an attempt stands; what a client
/// waiting out the 24 hours polls, and how it learns of a cancellation.
pub async fn poll(State(state): State<Arc<AppState>>, Json(h): Json<Handle>) -> ApiResult<Json<serde_json::Value>> {
    let a = open_attempt(&state.db, &h).await?;
    Ok(Json(json!({ "factor": a.factor, "ready": a.ready, "ready_at": a.ready_at })))
}

#[derive(Deserialize)]
pub struct VerifyRequest {
    #[serde(flatten)]
    pub handle: Handle,
    pub code: String,
}

/// `POST /v1/auth/recover/verify` -- the TOTP or emailed code.
pub async fn verify(State(state): State<Arc<AppState>>, Json(req): Json<VerifyRequest>) -> ApiResult<Json<serde_json::Value>> {
    let a = open_attempt(&state.db, &req.handle).await?;
    if a.ready {
        return Ok(Json(json!({ "ready": true })));
    }
    let ok = match a.factor.as_str() {
        "totp" => {
            let (secret,): (Option<Vec<u8>>,) = sqlx::query_as("SELECT totp_secret FROM users WHERE id = $1")
                .bind(a.user_id)
                .fetch_one(&state.db)
                .await?;
            secret.is_some_and(|s| security::code_matches(&s, &req.code))
        }
        "email" => {
            // The emailed code lasts a quarter of an hour from the start.
            let (fresh,): (bool,) = sqlx::query_as(
                "SELECT created_at > now() - ($2 || ' minutes')::interval FROM recovery_requests WHERE id = $1",
            )
            .bind(req.handle.request)
            .bind(EMAIL_CODE_MINUTES.to_string())
            .fetch_one(&state.db)
            .await?;
            fresh && a.code_hash.as_deref() == Some(code_hash(req.handle.request, req.code.trim()).as_slice())
        }
        _ => false,
    };
    if !ok {
        let attempts = a.attempts + 1;
        sqlx::query(
            "UPDATE recovery_requests SET attempts = $2, \
                    cancelled_at = CASE WHEN $2 >= $3 THEN now() END WHERE id = $1",
        )
        .bind(req.handle.request)
        .bind(attempts)
        .bind(MAX_CODE_ATTEMPTS)
        .execute(&state.db)
        .await?;
        return Err(ApiError::Refused("wrong_code"));
    }
    sqlx::query("UPDATE recovery_requests SET ready_at = now(), code_hash = NULL WHERE id = $1")
        .bind(req.handle.request)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ready": true })))
}

/// `POST /v1/auth/recover/material` -- the wrapped key and a challenge sealed
/// to the account's public key, once the second step is behind it.
pub async fn material(State(state): State<Arc<AppState>>, Json(h): Json<Handle>) -> ApiResult<Json<serde_json::Value>> {
    let a = open_attempt(&state.db, &h).await?;
    if !a.ready {
        return Err(ApiError::Refused("recovery_not_ready"));
    }
    let (public_key, salt, wrapped): (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT public_key, recovery_salt, recovery_wrapped_key FROM users WHERE id = $1",
    )
    .bind(a.user_id)
    .fetch_one(&state.db)
    .await?;
    let (Some(salt), Some(wrapped)) = (salt, wrapped) else { return Err(ApiError::Refused("recovery_closed")) };
    let public: [u8; 32] = public_key.as_slice().try_into().map_err(|_| ApiError::Internal(anyhow::anyhow!("a stored public key is not 32 bytes")))?;

    let nonce = fury_shared::keys::new_org_key();
    let sealed = fury_shared::keys::seal_to(&public, &nonce).map_err(|e| ApiError::Internal(anyhow::anyhow!(e.to_string())))?;
    let digest = {
        use sha2::{Digest, Sha256};
        Sha256::digest(nonce).to_vec()
    };
    sqlx::query("UPDATE recovery_requests SET challenge_hash = $2 WHERE id = $1")
        .bind(h.request)
        .bind(&digest)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({
        "public_key": B64.encode(&public_key),
        "salt": B64.encode(&salt),
        "wrapped_key": B64.encode(&wrapped),
        "challenge": B64.encode(&sealed),
    })))
}

#[derive(Deserialize)]
pub struct FinishRequest {
    #[serde(flatten)]
    pub handle: Handle,
    /// The challenge, opened: the proof of the private key.
    pub answer: String,
    pub password: String,
    pub kdf_salt: String,
    pub wrapped_private_key: String,
    /// The new recovery key's material. Required: the spent key ends here.
    pub recovery: KeyMaterial,
    #[serde(default)]
    pub machine_name: String,
    #[serde(default)]
    pub machine_id: Option<String>,
}

/// `POST /v1/auth/recover/finish`
pub async fn finish(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<FinishRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let ip = security::client_ip(&headers);
    let ua = security::user_agent(&headers);
    let a = open_attempt(&state.db, &req.handle).await?;
    let Some(expected) = a.challenge_hash else { return Err(ApiError::Refused("recovery_not_ready")) };
    let answer = decode("answer", &req.answer)?;
    let got = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&answer).to_vec()
    };
    if !a.ready || got != expected {
        return Err(ApiError::Refused("wrong_answer"));
    }
    if req.password.chars().count() < 8 {
        return Err(ApiError::BadRequest("the password must be at least 8 characters".into()));
    }
    let kdf_salt = sized("kdf_salt", &req.kdf_salt, fury_shared::keys::SALT_LEN)?;
    let wrapped_private_key = sized("wrapped_private_key", &req.wrapped_private_key, WRAPPED_KEY_LEN)?;
    let next = check_material(&req.recovery)?;
    let password_hash = auth::hash_password(&req.password).map_err(|e| ApiError::Internal(anyhow::anyhow!(e.to_string())))?;

    let mut tx = state.db.begin().await?;
    // Claimed first, so two finishes of one attempt cannot both land.
    let claimed = sqlx::query(
        "UPDATE recovery_requests SET completed_at = now() \
         WHERE id = $1 AND completed_at IS NULL AND cancelled_at IS NULL",
    )
    .bind(req.handle.request)
    .execute(&mut *tx)
    .await?;
    if claimed.rows_affected() != 1 {
        return Err(ApiError::Refused("recovery_closed"));
    }
    sqlx::query(
        "UPDATE users SET password_hash = $2, kdf_salt = $3, wrapped_private_key = $4, \
                recovery_salt = $5, recovery_wrapped_key = $6, recovery_auth_hash = $7, recovery_created_at = now() \
         WHERE id = $1",
    )
    .bind(a.user_id)
    .bind(&password_hash)
    .bind(&kdf_salt)
    .bind(&wrapped_private_key)
    .bind(&next.salt)
    .bind(&next.wrapped)
    .bind(&next.auth_hash)
    .execute(&mut *tx)
    .await?;
    // Every machine signed in with the old password signs in again.
    sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(a.user_id).execute(&mut *tx).await?;
    tx.commit().await?;

    let body = crate::api::issue_session(&state, a.user_id, &req.machine_name, req.machine_id.as_deref(), ip, ua.as_deref()).await?;
    let org = security::org_of(&state.db, a.user_id).await;
    let (email,): (String,) = sqlx::query_as("SELECT email::text FROM users WHERE id = $1")
        .bind(a.user_id)
        .fetch_one(&state.db)
        .await?;
    security::record_login(&state.db, Attempt {
        org, user: Some(a.user_id), email: &email, outcome: "recovered", ip,
        machine_name: &req.machine_name, machine_id: req.machine_id.as_deref(), user_agent: ua.as_deref(),
    }).await;
    if let Some(o) = org {
        security::audit_login_event(&state.db, o, a.user_id, "account.recovered", json!({
            "factor": a.factor, "ip": ip.map(|i| i.to_string()), "machine_name": req.machine_name,
        })).await;
    }
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// owner hand-over
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct HandOverRequest {
    pub user_id: Uuid,
    pub password: String,
    #[serde(default)]
    pub totp_code: Option<String>,
}

/// `POST /v1/org/owner` -- the owner makes an admin the owner, and becomes an
/// admin. The only way the owner of a team changes; `set_member_role` still
/// refuses the role. Only to an admin already let in, because an owner without
/// the organisation key could open nothing of the team they now own.
pub async fn hand_over(mut db: Db, Json(req): Json<HandOverRequest>) -> ApiResult<Json<serde_json::Value>> {
    use fury_shared::rbac::OrgRole;
    let caller = db.caller;
    if !matches!(caller.role, OrgRole::Owner) {
        return Err(ApiError::Refused("owner_only"));
    }
    if req.user_id == caller.user_id {
        return Err(ApiError::BadRequest("you are the owner already".into()));
    }
    reauthenticate(db.as_mut(), caller.user_id, &req.password, req.totp_code.as_deref()).await?;
    let target: Option<(String, bool)> = sqlx::query_as(
        "SELECT role::text, wrapped_ork IS NOT NULL FROM org_members WHERE org_id = $1 AND user_id = $2",
    )
    .bind(caller.org_id)
    .bind(req.user_id)
    .fetch_optional(db.as_mut())
    .await?;
    match target {
        None => return Err(ApiError::NotFound),
        Some((role, _)) if role != "admin" => return Err(ApiError::Refused("not_an_admin")),
        Some((_, false)) => return Err(ApiError::Refused("not_let_in")),
        _ => {}
    }
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE org_members SET role = 'admin' WHERE org_id = $1 AND user_id = $2")
        .bind(caller.org_id)
        .bind(caller.user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE org_members SET role = 'owner' WHERE org_id = $1 AND user_id = $2")
        .bind(caller.org_id)
        .bind(req.user_id)
        .execute(&mut *tx)
        .await?;
    crate::api::audit(&mut *tx, &caller, "org.owner_transferred", Some(req.user_id), json!({ "to": req.user_id })).await?;
    tx.commit().await?;
    Ok(Json(json!({ "owner": req.user_id })))
}
