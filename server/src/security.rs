// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Team security: the organisation's sign-in policy, the second factor, the
//! login journal, and sessions as things an owner can see and end.
//!
//! What AdsPower and GoLogin sell to agencies under "account security", and
//! what server/src had none of until 12.09.2026 (docs/08 §4, docs/16 5.3):
//!
//! - **an IP allowlist for sign-in**, per organisation, owners exempt;
//! - **a second factor**: TOTP, the one every authenticator speaks and the one
//!   shared-rs already implements for the credential store. Required never,
//!   on a device the user has not signed in from before, or always — the
//!   organisation chooses;
//! - **a login journal** with outcome, address and device, including the
//!   failures, which is where password guessing shows;
//! - **a new-device notice** as an audit event — this server sends no mail, so
//!   the audit screen is where the owner sees it;
//! - **sessions listed and revocable**: your own from other machines, or, for
//!   an owner, everyone's.
//!
//! Not here, and named: a second factor on individual sensitive actions
//! (delete a profile, remove a member). AdsPower has it; it needs a step-up
//! flow through every such handler and a place in the desktop to ask for the
//! code, and is recorded in docs/16 rather than half-built.

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::api::{audit, rfc3339};
use crate::auth::{self, Db};
use crate::error::{ApiError, ApiResult};
use crate::AppState;
use fury_shared::rbac::{OrgRole, Perm};

// ---------------------------------------------------------------------------
// policy
// ---------------------------------------------------------------------------

/// What the organisation requires at sign-in. Stored as JSON on the
/// organisation; every field has a default so an old row reads as "nothing
/// required", which is what it was.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// `off`, `new_device` or `always`.
    #[serde(default = "default_second_factor")]
    pub second_factor: String,
    /// Addresses or CIDR ranges sign-in is accepted from. Empty means any.
    #[serde(default)]
    pub ip_allowlist: Vec<String>,
    /// Owners sign in from anywhere regardless. On by default: an owner who
    /// locks themselves out of their own server with a typo has nobody to call.
    #[serde(default = "default_true")]
    pub owner_exempt_from_allowlist: bool,
    /// Ask for a fresh code before the actions that cannot be undone —
    /// purging a profile, removing a member, changing this policy, ending
    /// everyone's sessions. A session with no second factor enrolled is
    /// refused those actions outright while this is on: the owner turning it
    /// on is shown who has not enrolled.
    #[serde(default)]
    pub sensitive_actions_2fa: bool,
}

fn default_second_factor() -> String {
    "off".into()
}
fn default_true() -> bool {
    true
}

impl Default for Policy {
    fn default() -> Self {
        Self { second_factor: "off".into(), ip_allowlist: Vec::new(), owner_exempt_from_allowlist: true, sensitive_actions_2fa: false }
    }
}

impl Policy {
    pub fn from_json(v: &serde_json::Value) -> Self {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }

    /// Is this address allowed to sign in under this policy, for this role?
    pub fn admits(&self, ip: Option<IpAddr>, role: OrgRole) -> bool {
        if self.ip_allowlist.is_empty() {
            return true;
        }
        if self.owner_exempt_from_allowlist && matches!(role, OrgRole::Owner) {
            return true;
        }
        // No address at all — a request that arrived with no forwarding
        // header — is refused when a list exists: "unknown" is not "allowed".
        let Some(ip) = ip else { return false };
        self.ip_allowlist.iter().any(|entry| cidr_contains(entry, ip))
    }

    /// Reject a policy that would lock everyone out or that names ranges
    /// that do not parse — at write time, with the entry named, rather than at
    /// the next sign-in with nothing named.
    pub fn validate(&self) -> Result<(), String> {
        if !["off", "new_device", "always"].contains(&self.second_factor.as_str()) {
            return Err(format!("second_factor must be off, new_device or always, not {:?}", self.second_factor));
        }
        for e in &self.ip_allowlist {
            if parse_cidr(e).is_none() {
                return Err(format!("{e:?} is not an address or a CIDR range"));
            }
        }
        Ok(())
    }
}

/// `203.0.113.7`, `203.0.113.0/24`, `2001:db8::/32` → (network, prefix bits).
fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let s = s.trim();
    let (addr, bits) = match s.split_once('/') {
        Some((a, b)) => (a.parse::<IpAddr>().ok()?, b.parse::<u8>().ok()?),
        None => {
            let a = s.parse::<IpAddr>().ok()?;
            (a, if a.is_ipv4() { 32 } else { 128 })
        }
    };
    let max = if addr.is_ipv4() { 32 } else { 128 };
    (bits <= max).then_some((addr, bits))
}

fn cidr_contains(entry: &str, ip: IpAddr) -> bool {
    let Some((net, bits)) = parse_cidr(entry) else { return false };
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(a)) => {
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            (u32::from(n) & mask) == (u32::from(a) & mask)
        }
        (IpAddr::V6(n), IpAddr::V6(a)) => {
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            (u128::from(n) & mask) == (u128::from(a) & mask)
        }
        // A v4 address arriving as ::ffff:a.b.c.d against a v4 entry.
        (IpAddr::V4(_), IpAddr::V6(a)) => a.to_ipv4_mapped().map(|m| cidr_contains(entry, IpAddr::V4(m))).unwrap_or(false),
        _ => false,
    }
}

/// The client's address as the reverse proxy reported it.
///
/// The server binds loopback and Caddy fronts it (deploy/server-install.sh),
/// so the first `X-Forwarded-For` entry is the client and the peer address is
/// always 127.0.0.1. A deployment without a proxy gets `None` here and an
/// allowlist that refuses everyone — which is the right failure for a security
/// setting that cannot be evaluated.
pub fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
        .map(str::trim)
        .and_then(|s| s.parse().ok())
}

pub fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers.get(axum::http::header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|s| s.chars().take(200).collect())
}

/// The organisation's policy, by org id.
pub async fn policy_of(db: &mut sqlx::PgConnection, org: Uuid) -> ApiResult<Policy> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as("SELECT security FROM organizations WHERE id = $1")
        .bind(org)
        .fetch_optional(&mut *db)
        .await?;
    Ok(row.map(|(v,)| Policy::from_json(&v)).unwrap_or_default())
}

// ---------------------------------------------------------------------------
// the login journal
// ---------------------------------------------------------------------------

pub struct Attempt<'a> {
    pub org: Option<Uuid>,
    pub user: Option<Uuid>,
    pub email: &'a str,
    pub outcome: &'a str,
    pub ip: Option<IpAddr>,
    pub machine_name: &'a str,
    pub machine_id: Option<&'a str>,
    pub user_agent: Option<&'a str>,
}

/// One line in the journal. Never fails the sign-in: a journal that could
/// block a login would be a way to block logins.
pub async fn record_login(db: &sqlx::PgPool, a: Attempt<'_>) {
    // The address travels as text and is cast in SQL: sqlx's inet type needs
    // a feature and a crate for one column.
    let r = sqlx::query("SELECT record_login($1, $2, $3, $4, $5::inet, $6, $7, $8)")
        .bind(a.org)
        .bind(a.user)
        .bind(a.email)
        .bind(a.outcome)
        .bind(a.ip.map(|ip| ip.to_string()))
        .bind(a.machine_name)
        .bind(a.machine_id)
        .bind(a.user_agent)
        .execute(db)
        .await;
    if let Err(e) = r {
        tracing::error!(error = %e, "login journal write failed");
    }
}

/// A count over the journal, read as the user it concerns.
///
/// The journal is under FORCE ROW LEVEL SECURITY, and an unbound pool
/// connection sees an empty table — which is the right failure for a handler
/// that forgot to bind, and the wrong answer here: the first end-to-end run
/// counted zero earlier sign-ins for a device that had two, and called every
/// sign-in a new device. Bound as the user, whose organisation the rows
/// belong to, the count is real. The after_release hook clears the binding.
async fn journal_count(db: &sqlx::PgPool, user: Uuid, sql: &str, arg: &str) -> i64 {
    let r: Result<i64, sqlx::Error> = async {
        let mut conn = db.acquire().await?;
        sqlx::query("SELECT set_config('app.user_id', $1, false)")
            .bind(user.to_string())
            .execute(&mut *conn)
            .await?;
        let (n,): (i64,) = sqlx::query_as(sql).bind(user).bind(arg).fetch_one(&mut *conn).await?;
        Ok(n)
    }
    .await;
    r.unwrap_or(0)
}

/// Has this user signed in from this device before?
pub async fn known_device(db: &sqlx::PgPool, user: Uuid, machine_id: Option<&str>) -> bool {
    let Some(mid) = machine_id.filter(|m| !m.is_empty()) else {
        // A client that sends no id is treated as a new device every time.
        return false;
    };
    journal_count(
        db,
        user,
        "SELECT count(*) FROM login_events WHERE user_id = $1 AND machine_id = $2 AND outcome = 'ok'",
        mid,
    )
    .await
        > 0
}

/// Three or more failed passwords for one address in the last quarter hour.
pub async fn failure_burst(db: &sqlx::PgPool, user: Uuid, email: &str) -> bool {
    journal_count(
        db,
        user,
        "SELECT count(*) FROM login_events WHERE user_id = $1 AND email = $2 AND outcome = 'bad_password' \
         AND at > now() - interval '15 minutes'",
        email,
    )
    .await
        >= 3
}

/// An audit row written without a `Caller` — at sign-in there is none yet.
///
/// The connection is bound to the user for the one statement: audit_events is
/// under FORCE ROW LEVEL SECURITY and its policy is `user_in_org(org_id)`, so
/// an unbound pool connection is refused — which the end-to-end run of
/// 12.09.2026 showed as "new row violates row-level security policy" and an
/// empty sign-in section of the audit. The user is a member of the
/// organisation the row names, so bound as them the policy admits it, and
/// `main::connect`'s after_release hook clears the binding on the way back.
pub async fn audit_login_event(db: &sqlx::PgPool, org: Uuid, user: Uuid, action: &str, detail: serde_json::Value) {
    let r: Result<(), sqlx::Error> = async {
        let mut conn = db.acquire().await?;
        sqlx::query("SELECT set_config('app.user_id', $1, false)")
            .bind(user.to_string())
            .execute(&mut *conn)
            .await?;
        sqlx::query(
            "INSERT INTO audit_events (org_id, actor_user_id, action, target_id, detail) VALUES ($1, $2, $3, $2, $4)",
        )
        .bind(org)
        .bind(user)
        .bind(action)
        .bind(detail)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        tracing::error!(error = %e, action, "audit write failed");
    }
}

// ---------------------------------------------------------------------------
// the second factor at sign-in
// ---------------------------------------------------------------------------

/// Does this sign-in need a code before it gets a session?
pub fn second_factor_required(policy: &Policy, enrolled: bool, known_device: bool) -> bool {
    if !enrolled {
        // Nothing to check against. The policy is enforced through
        // `must_enrol` on the session instead: the client is told, and the
        // organisation's screen shows who has not enrolled.
        return false;
    }
    match policy.second_factor.as_str() {
        "always" => true,
        "new_device" => !known_device,
        _ => false,
    }
}

/// Park a sign-in that passed the password until the code arrives.
pub async fn open_challenge(
    db: &sqlx::PgPool,
    user: Uuid,
    ip: Option<IpAddr>,
    machine_name: &str,
    machine_id: Option<&str>,
    ua: Option<&str>,
) -> ApiResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO login_challenges (id, user_id, ip, machine_name, machine_id, user_agent, expires_at) \
         VALUES ($1, $2, $3::inet, $4, $5, $6, now() + interval '5 minutes')",
    )
    .bind(id)
    .bind(user)
    .bind(ip.map(|ip| ip.to_string()))
    .bind(machine_name)
    .bind(machine_id)
    .bind(ua)
    .execute(db)
    .await?;
    Ok(id)
}

#[derive(Deserialize)]
pub struct TotpLoginRequest {
    challenge: Uuid,
    code: String,
}

/// `POST /v1/auth/login/totp` — the second half of a sign-in.
pub async fn login_totp(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TotpLoginRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // The address and device come from the challenge row, not from this
    // request: they were recorded when the password was checked, and a code
    // presented from a different address than the password is worth seeing
    // in the journal as the address that started it.
    let row: Option<(Uuid, Option<String>, String, Option<String>, Option<String>, i32)> =
        sqlx::query_as(
            "SELECT user_id, host(ip), machine_name, machine_id, user_agent, attempts FROM login_challenges \
             WHERE id = $1 AND expires_at > now()",
        )
        .bind(req.challenge)
        .fetch_optional(&state.db)
        .await?;
    let Some((user_id, ip, machine_name, machine_id, ua, attempts)) = row else {
        return Err(ApiError::Unauthenticated);
    };
    // Five wrong codes end the challenge: a six-digit code with a thirty
    // second window survives a handful of guesses and not a thousand.
    if attempts >= 5 {
        sqlx::query("DELETE FROM login_challenges WHERE id = $1").bind(req.challenge).execute(&state.db).await?;
        return Err(ApiError::Unauthenticated);
    }

    let secret: Option<(Option<Vec<u8>>, String)> =
        sqlx::query_as("SELECT totp_secret, email FROM users WHERE id = $1 AND totp_enabled_at IS NOT NULL")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((Some(secret), email)) = secret else { return Err(ApiError::Unauthenticated) };
    let ip_addr: Option<IpAddr> = ip.and_then(|s| s.parse().ok());

    if !code_matches(&secret, &req.code) {
        sqlx::query("UPDATE login_challenges SET attempts = attempts + 1 WHERE id = $1")
            .bind(req.challenge)
            .execute(&state.db)
            .await?;
        let org = org_of(&state.db, user_id).await;
        record_login(&state.db, Attempt {
            org, user: Some(user_id), email: &email, outcome: "totp_failed", ip: ip_addr,
            machine_name: &machine_name, machine_id: machine_id.as_deref(), user_agent: ua.as_deref(),
        }).await;
        return Err(ApiError::Unauthenticated);
    }

    sqlx::query("DELETE FROM login_challenges WHERE id = $1").bind(req.challenge).execute(&state.db).await?;
    let body = crate::api::issue_session(&state, user_id, &machine_name, machine_id.as_deref(), ip_addr, ua.as_deref()).await?;
    let org = org_of(&state.db, user_id).await;
    let fresh = !known_device(&state.db, user_id, machine_id.as_deref()).await;
    record_login(&state.db, Attempt {
        org, user: Some(user_id), email: &email, outcome: "ok", ip: ip_addr,
        machine_name: &machine_name, machine_id: machine_id.as_deref(), user_agent: ua.as_deref(),
    }).await;
    if let (Some(org), true) = (org, fresh) {
        audit_login_event(&state.db, org, user_id, "login.new_device", json!({
            "machine_name": machine_name, "ip": ip_addr.map(|i| i.to_string()), "second_factor": true
        })).await;
    }
    Ok(Json(body))
}

pub async fn org_of(db: &sqlx::PgPool, user: Uuid) -> Option<Uuid> {
    sqlx::query_as::<_, (Uuid,)>("SELECT org_id FROM org_members WHERE user_id = $1 LIMIT 1")
        .bind(user)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .map(|(o,)| o)
}

/// This code, or the one before or after: a clock a few seconds out is not a
/// wrong password.
pub(crate) fn code_matches(secret: &[u8], code: &str) -> bool {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let totp = fury_shared::totp::Totp {
        secret: secret.to_vec(),
        digits: 6,
        period: 30,
        algorithm: fury_shared::totp::Algorithm::Sha1,
        label: None,
        issuer: None,
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    [now.saturating_sub(30), now, now + 30].iter().any(|t| totp.code_at(*t) == code)
}

// ---------------------------------------------------------------------------
// step-up: a fresh code before an action that cannot be undone
// ---------------------------------------------------------------------------

/// How long a code keeps a session marked as recently verified.
const STEP_UP_WINDOW_MINUTES: i64 = 10;

/// Refuse unless the organisation does not ask for it, or this session
/// presented a code within the window. Called at the top of the sensitive
/// handlers with the request headers, since the session is what carries the
/// mark and the `Caller` does not know which session it is.
pub async fn require_step_up(db: &mut sqlx::PgConnection, caller: &crate::auth::Caller, headers: &HeaderMap) -> ApiResult<()> {
    let policy = policy_of(db, caller.org_id).await?;
    if !policy.sensitive_actions_2fa {
        return Ok(());
    }
    let enrolled: (bool,) = sqlx::query_as("SELECT totp_enabled_at IS NOT NULL FROM users WHERE id = $1")
        .bind(caller.user_id)
        .fetch_one(&mut *db)
        .await?;
    if !enrolled.0 {
        return Err(ApiError::Refused("totp_enrolment_required"));
    }
    let Some(hash) = auth::token_hash_from_headers(headers) else {
        return Err(ApiError::Unauthenticated);
    };
    let fresh: Option<(bool,)> = sqlx::query_as(
        "SELECT coalesce(totp_verified_at > now() - ($2 || ' minutes')::interval, false) FROM sessions WHERE token_hash = $1",
    )
    .bind(&hash)
    .bind(STEP_UP_WINDOW_MINUTES.to_string())
    .fetch_optional(&mut *db)
    .await?;
    match fresh {
        Some((true,)) => Ok(()),
        _ => Err(ApiError::Refused("step_up_required")),
    }
}

/// `POST /v1/me/totp/verify` — marks this session as recently verified.
pub async fn totp_verify(mut db: Db, headers: HeaderMap, Json(req): Json<CodeRequest>) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let row: Option<(Option<Vec<u8>>,)> = sqlx::query_as("SELECT totp_secret FROM users WHERE id = $1 AND totp_enabled_at IS NOT NULL")
        .bind(caller.user_id)
        .fetch_optional(db.as_mut())
        .await?;
    let Some((Some(secret),)) = row else {
        return Err(ApiError::Refused("totp_enrolment_required"));
    };
    if !code_matches(&secret, &req.code) {
        return Err(ApiError::BadRequest("that code does not match".into()));
    }
    let Some(hash) = auth::token_hash_from_headers(&headers) else {
        return Err(ApiError::Unauthenticated);
    };
    sqlx::query("UPDATE sessions SET totp_verified_at = now() WHERE token_hash = $1")
        .bind(&hash)
        .execute(db.as_mut())
        .await?;
    Ok(Json(json!({ "verified": true, "minutes": STEP_UP_WINDOW_MINUTES })))
}

// ---------------------------------------------------------------------------
// enrolling the second factor
// ---------------------------------------------------------------------------

/// `GET /v1/me/totp`
pub async fn totp_status(mut db: Db) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let row: Option<(Option<Vec<u8>>, Option<String>)> = sqlx::query_as(&format!(
        "SELECT totp_secret, {} FROM users WHERE id = $1",
        rfc3339("totp_enabled_at")
    ))
    .bind(caller.user_id)
    .fetch_optional(db.as_mut())
    .await?;
    let (secret, enabled_at) = row.unwrap_or((None, None));
    let policy = policy_of(db.as_mut(), caller.org_id).await?;
    Ok(Json(json!({
        "enabled": enabled_at.is_some(),
        "enabled_at": enabled_at,
        "pending": secret.is_some() && enabled_at.is_none(),
        "required_by_org": policy.second_factor,
    })))
}

/// `POST /v1/me/totp/setup` — a new secret, pending until a code confirms it.
/// Returns the otpauth URI the authenticator scans and the bare secret for
/// typing. Calling it again replaces a pending secret and never an enabled one.
pub async fn totp_setup(mut db: Db) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let enabled: Option<(Option<String>,)> = sqlx::query_as(&format!(
        "SELECT {} FROM users WHERE id = $1",
        rfc3339("totp_enabled_at")
    ))
    .bind(caller.user_id)
    .fetch_optional(db.as_mut())
    .await?;
    if matches!(enabled, Some((Some(_),))) {
        return Err(ApiError::Conflict("a second factor is already enabled; disable it first".into()));
    }
    let secret: [u8; 20] = rand::random();
    sqlx::query("UPDATE users SET totp_secret = $2 WHERE id = $1")
        .bind(caller.user_id)
        .bind(&secret[..])
        .execute(db.as_mut())
        .await?;
    let email: (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(caller.user_id)
        .fetch_one(db.as_mut())
        .await?;
    let totp = fury_shared::totp::Totp {
        secret: secret.to_vec(),
        digits: 6,
        period: 30,
        algorithm: fury_shared::totp::Algorithm::Sha1,
        label: Some(email.0.clone()),
        issuer: Some("Fury".into()),
    };
    Ok(Json(json!({ "uri": totp.to_uri(), "secret": base32(&secret) })))
}

#[derive(Deserialize)]
pub struct CodeRequest {
    code: String,
}

/// `POST /v1/me/totp/confirm` — proves the authenticator has the secret.
pub async fn totp_confirm(mut db: Db, Json(req): Json<CodeRequest>) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let row: Option<(Option<Vec<u8>>,)> = sqlx::query_as("SELECT totp_secret FROM users WHERE id = $1 AND totp_enabled_at IS NULL")
        .bind(caller.user_id)
        .fetch_optional(db.as_mut())
        .await?;
    let Some((Some(secret),)) = row else {
        return Err(ApiError::BadRequest("no pending second factor to confirm".into()));
    };
    if !code_matches(&secret, &req.code) {
        return Err(ApiError::BadRequest("that code does not match — check the clock on the phone".into()));
    }
    sqlx::query("UPDATE users SET totp_enabled_at = now() WHERE id = $1")
        .bind(caller.user_id)
        .execute(db.as_mut())
        .await?;
    audit(db.as_mut(), &caller, "user.totp_enabled", Some(caller.user_id), json!({})).await?;
    Ok(Json(json!({ "enabled": true })))
}

/// `DELETE /v1/me/totp` — needs a current code, so a stolen session cannot
/// quietly remove the factor.
pub async fn totp_disable(mut db: Db, Json(req): Json<CodeRequest>) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let row: Option<(Option<Vec<u8>>,)> = sqlx::query_as("SELECT totp_secret FROM users WHERE id = $1")
        .bind(caller.user_id)
        .fetch_optional(db.as_mut())
        .await?;
    let Some((Some(secret),)) = row else {
        return Ok(Json(json!({ "enabled": false })));
    };
    if !code_matches(&secret, &req.code) {
        return Err(ApiError::BadRequest("that code does not match".into()));
    }
    sqlx::query("UPDATE users SET totp_secret = NULL, totp_enabled_at = NULL WHERE id = $1")
        .bind(caller.user_id)
        .execute(db.as_mut())
        .await?;
    audit(db.as_mut(), &caller, "user.totp_disabled", Some(caller.user_id), json!({})).await?;
    Ok(Json(json!({ "enabled": false })))
}

fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &b in bytes {
        buffer = (buffer << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// the organisation's policy
// ---------------------------------------------------------------------------

/// `GET /v1/org/security` — owners and admins.
pub async fn get_policy(mut db: Db) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }
    let policy = policy_of(db.as_mut(), caller.org_id).await?;
    // Who has the factor and who has not: the screen that sets "always" needs
    // to show who it is about to lock out at their next sign-in.
    let members: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(&format!(
        "SELECT u.id, u.email, {} FROM org_members m JOIN users u ON u.id = m.user_id WHERE m.org_id = $1 ORDER BY u.email",
        rfc3339("u.totp_enabled_at")
    ))
    .bind(caller.org_id)
    .fetch_all(db.as_mut())
    .await?;
    Ok(Json(json!({
        "policy": policy,
        "members": members.into_iter().map(|(id, email, at)| json!({ "user_id": id, "email": email, "totp_enabled_at": at })).collect::<Vec<_>>(),
    })))
}

/// `PUT /v1/org/security` — owners only. Validated before it is written.
pub async fn put_policy(mut db: Db, headers: HeaderMap, Json(policy): Json<Policy>) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    if !matches!(caller.role, OrgRole::Owner) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }
    require_step_up(db.as_mut(), &caller, &headers).await?;
    policy.validate().map_err(ApiError::BadRequest)?;
    // Refuse a list that would refuse the owner writing it, unless they are
    // exempt: the next request from this address would be their last.
    if !policy.admits(client_ip(&headers), caller.role) {
        return Err(ApiError::BadRequest(
            "this allowlist does not include the address you are connected from, and owners are not exempt in it".into(),
        ));
    }
    // The same courtesy for the code: an owner who turns this on without a
    // second factor of their own could never turn it off again.
    if policy.sensitive_actions_2fa {
        let enrolled: (bool,) = sqlx::query_as("SELECT totp_enabled_at IS NOT NULL FROM users WHERE id = $1")
            .bind(caller.user_id)
            .fetch_one(db.as_mut())
            .await?;
        if !enrolled.0 {
            return Err(ApiError::Refused("totp_enrolment_required"));
        }
    }
    sqlx::query("UPDATE organizations SET security = $2 WHERE id = $1")
        .bind(caller.org_id)
        .bind(serde_json::to_value(&policy).unwrap_or_default())
        .execute(db.as_mut())
        .await?;
    audit(db.as_mut(), &caller, "org.security_changed", None, serde_json::to_value(&policy).unwrap_or_default()).await?;
    Ok(Json(json!({ "policy": policy })))
}

// ---------------------------------------------------------------------------
// journals
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginsQuery {
    limit: Option<i64>,
    before: Option<i64>,
    /// Only this outcome, e.g. `bad_password`.
    outcome: Option<String>,
    email: Option<String>,
}

#[derive(Serialize)]
pub struct LoginEntry {
    id: i64,
    email: String,
    outcome: String,
    ip: Option<String>,
    machine_name: String,
    user_agent: Option<String>,
    at: String,
}

/// `GET /v1/audit/logins` — owners and admins. RLS scopes it to the org.
pub async fn list_logins(mut db: Db, Query(q): Query<LoginsQuery>) -> ApiResult<Json<Vec<LoginEntry>>> {
    let caller = db.caller;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let rows: Vec<(i64, String, String, Option<String>, String, Option<String>, String)> =
        sqlx::query_as(&format!(
            "SELECT id, email, outcome, host(ip), machine_name, user_agent, {} FROM login_events \
             WHERE org_id = $1 AND ($2::bigint IS NULL OR id < $2) \
               AND ($4::text IS NULL OR outcome = $4) AND ($5::text IS NULL OR email = $5) \
             ORDER BY id DESC LIMIT $3",
            rfc3339("at")
        ))
        .bind(caller.org_id)
        .bind(q.before)
        .bind(limit)
        .bind(q.outcome.filter(|s| !s.is_empty()))
        .bind(q.email.filter(|s| !s.is_empty()))
        .fetch_all(db.as_mut())
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, email, outcome, ip, machine_name, user_agent, at)| LoginEntry {
                id, email, outcome, ip, machine_name, user_agent, at,
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// sessions
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SessionEntry {
    /// The first eight hex characters of the token hash: enough to name a row
    /// in a revoke call, useless for anything else.
    id: String,
    user_id: Uuid,
    email: String,
    machine_name: String,
    ip: Option<String>,
    user_agent: Option<String>,
    created_at: String,
    last_seen_at: String,
    /// The session making this request.
    current: bool,
}

/// `GET /v1/sessions` — your own; `?all=1` for owners and admins, everyone's.
pub async fn list_sessions(mut db: Db, headers: HeaderMap, Query(q): Query<std::collections::HashMap<String, String>>) -> ApiResult<Json<Vec<SessionEntry>>> {
    let caller = db.caller;
    let all = q.get("all").map(|v| v == "1" || v == "true").unwrap_or(false);
    if all && !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }
    let mine = auth::token_hash_from_headers(&headers);
    let rows: Vec<(Vec<u8>, Uuid, String, String, Option<String>, Option<String>, String, String)> =
        sqlx::query_as(&format!(
            "SELECT s.token_hash, s.user_id, u.email, s.machine_name, host(s.ip), s.user_agent, {}, {} \
             FROM sessions s JOIN users u ON u.id = s.user_id JOIN org_members m ON m.user_id = s.user_id \
             WHERE s.expires_at > now() AND m.org_id = $1 AND ($2::boolean OR s.user_id = $3) \
             ORDER BY s.last_seen_at DESC",
            rfc3339("s.created_at"),
            rfc3339("s.last_seen_at")
        ))
        .bind(caller.org_id)
        .bind(all)
        .bind(caller.user_id)
        .fetch_all(db.as_mut())
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(hash, user_id, email, machine_name, ip, ua, created_at, last_seen_at)| SessionEntry {
                current: mine.as_deref() == Some(hash.as_slice()),
                id: hex_prefix(&hash),
                user_id, email, machine_name,
                ip,
                user_agent: ua, created_at, last_seen_at,
            })
            .collect(),
    ))
}

fn hex_prefix(hash: &[u8]) -> String {
    hash.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// `DELETE /v1/sessions/{id}` — one session, by its prefix. Your own always;
/// somebody else's for owners and admins. The session ends at the next request
/// it makes, which for a desktop polling every few seconds is now.
pub async fn revoke_session(mut db: Db, Path(id): Path<String>) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    if id.len() != 8 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::BadRequest("a session id is eight hex characters".into()));
    }
    let manages = matches!(caller.role, OrgRole::Owner | OrgRole::Admin);
    let rows: Vec<(Vec<u8>, Uuid, String)> = sqlx::query_as(
        "SELECT s.token_hash, s.user_id, s.machine_name FROM sessions s JOIN org_members m ON m.user_id = s.user_id \
         WHERE m.org_id = $1 AND encode(substring(s.token_hash from 1 for 4), 'hex') = $2",
    )
    .bind(caller.org_id)
    .bind(&id)
    .fetch_all(db.as_mut())
    .await?;
    let Some((hash, owner, machine)) = rows.into_iter().next() else {
        return Err(ApiError::NotFound);
    };
    if owner != caller.user_id && !manages {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1").bind(&hash).execute(db.as_mut()).await?;
    audit(db.as_mut(), &caller, "session.revoked", Some(owner), json!({ "machine_name": machine, "own": owner == caller.user_id })).await?;
    Ok(Json(json!({ "revoked": 1 })))
}

/// `DELETE /v1/org/members/{user_id}/sessions` — every session of one member.
/// What an owner does the moment a freelancer stops being one.
pub async fn revoke_member_sessions(mut db: Db, headers: HeaderMap, Path(user_id): Path<Uuid>) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }
    require_step_up(db.as_mut(), &caller, &headers).await?;
    let n = sqlx::query(
        "DELETE FROM sessions WHERE user_id = $1 AND user_id IN (SELECT user_id FROM org_members WHERE org_id = $2)",
    )
    .bind(user_id)
    .bind(caller.org_id)
    .execute(db.as_mut())
    .await?
    .rows_affected();
    audit(db.as_mut(), &caller, "session.revoked_all", Some(user_id), json!({ "count": n })).await?;
    Ok(Json(json!({ "revoked": n })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_matching_covers_both_families_and_bare_addresses() {
        assert!(cidr_contains("203.0.113.0/24", "203.0.113.77".parse().unwrap()));
        assert!(!cidr_contains("203.0.113.0/24", "203.0.114.1".parse().unwrap()));
        assert!(cidr_contains("198.51.100.7", "198.51.100.7".parse().unwrap()));
        assert!(!cidr_contains("198.51.100.7", "198.51.100.8".parse().unwrap()));
        assert!(cidr_contains("2001:db8::/32", "2001:db8:1::5".parse().unwrap()));
        assert!(cidr_contains("203.0.113.0/24", "::ffff:203.0.113.9".parse().unwrap()));
        assert!(!cidr_contains("not an address", "1.2.3.4".parse().unwrap()));
    }

    #[test]
    fn an_empty_allowlist_admits_everyone_and_owners_are_exempt() {
        let open = Policy::default();
        assert!(open.admits(None, OrgRole::Member));
        let strict = Policy { ip_allowlist: vec!["10.0.0.0/8".into()], ..Policy::default() };
        assert!(strict.admits(Some("10.1.2.3".parse().unwrap()), OrgRole::Member));
        assert!(!strict.admits(Some("8.8.8.8".parse().unwrap()), OrgRole::Member));
        assert!(!strict.admits(None, OrgRole::Member), "no address is not an allowed address");
        assert!(strict.admits(Some("8.8.8.8".parse().unwrap()), OrgRole::Owner));
        let no_exempt = Policy { owner_exempt_from_allowlist: false, ..strict };
        assert!(!no_exempt.admits(Some("8.8.8.8".parse().unwrap()), OrgRole::Owner));
    }

    #[test]
    fn a_policy_that_does_not_parse_is_refused_when_written() {
        assert!(Policy { second_factor: "sometimes".into(), ..Policy::default() }.validate().is_err());
        assert!(Policy { ip_allowlist: vec!["10.0.0.0/33".into()], ..Policy::default() }.validate().is_err());
        assert!(Policy { ip_allowlist: vec!["10.0.0.0/8".into(), "::1".into()], ..Policy::default() }.validate().is_ok());
    }

    #[test]
    fn the_second_factor_follows_the_policy_and_the_device() {
        let p = |s: &str| Policy { second_factor: s.into(), ..Policy::default() };
        assert!(!second_factor_required(&p("always"), false, false), "nothing to check against yet");
        assert!(second_factor_required(&p("always"), true, true));
        assert!(second_factor_required(&p("new_device"), true, false));
        assert!(!second_factor_required(&p("new_device"), true, true));
        assert!(!second_factor_required(&p("off"), true, false));
    }

    #[test]
    fn a_code_matches_its_window_and_nothing_else() {
        let secret = [7u8; 20];
        let totp = fury_shared::totp::Totp {
            secret: secret.to_vec(), digits: 6, period: 30,
            algorithm: fury_shared::totp::Algorithm::Sha1, label: None, issuer: None,
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(code_matches(&secret, &totp.code_at(now)));
        assert!(code_matches(&secret, &totp.code_at(now - 30)));
        assert!(!code_matches(&secret, &totp.code_at(now + 300)) || totp.code_at(now + 300) == totp.code_at(now));
        assert!(!code_matches(&secret, "12345"));
        assert!(!code_matches(&secret, "abcdef"));
    }

    #[test]
    fn client_ip_reads_the_first_forwarded_hop() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "203.0.113.5, 10.0.0.1".parse().unwrap());
        assert_eq!(client_ip(&h), Some("203.0.113.5".parse().unwrap()));
        assert_eq!(client_ip(&HeaderMap::new()), None);
    }

    #[test]
    fn base32_matches_the_rfc_vector() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    }
}
