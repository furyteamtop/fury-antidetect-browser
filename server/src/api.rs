//! HTTP handlers.
//!
//! Two rules hold everywhere in this module:
//!
//! 1. **Every handler resolves permissions before it touches data.** Never from
//!    a claim in the request, always from the database. The client is told what
//!    it may do so the UI can hide what it cannot, but that is presentation —
//!    the decision is made here.
//!
//! 2. **A caller who cannot see a project is told it does not exist.** See
//!    error.rs for why.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use fury_shared::api::{
    AcquireLockResponse, GrantRequest, LockInfo, ProfileSummary, ProjectSummary, ProxySummary,
};
use fury_shared::rbac::{effective, LaunchRestrictions, Perm, PermSet};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::{self, Caller};
use crate::error::{ApiError, ApiResult};
use crate::rbac_guard;
use crate::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/auth/login", post(login))
        .route("/v1/auth/logout", post(logout))
        .route("/v1/me", get(whoami))
        .route("/v1/projects", get(list_projects))
        .route("/v1/projects/{project_id}/profiles", get(list_profiles))
        .route("/v1/projects/{project_id}/grants", post(grant_access))
        .route("/v1/profiles/{profile_id}/lock", post(acquire_lock))
        .route("/v1/profiles/{profile_id}/lock/heartbeat", post(heartbeat))
        .route("/v1/profiles/{profile_id}/unlock", post(release_lock))
}

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginRequest {
    email: String,
    password: String,
    #[serde(default)]
    machine_name: String,
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let row: Option<(Uuid, String)> =
        sqlx::query_as("SELECT id, password_hash FROM users WHERE email = $1 AND disabled_at IS NULL")
            .bind(&req.email)
            .fetch_optional(&state.db)
            .await?;

    // One failure mode for "no such user" and "wrong password", or the endpoint
    // becomes a way to discover who has an account.
    let Some((user_id, stored)) = row else {
        // Still spend the time an Argon2 verification would take. Returning
        // instantly for unknown addresses is a timing oracle over the user list.
        let _ = auth::verify_password(&req.password, "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$0000000000000000000000000000000000000000000");
        return Err(ApiError::Unauthenticated);
    };
    if !auth::verify_password(&req.password, &stored) {
        return Err(ApiError::Unauthenticated);
    }

    let (raw, digest) = auth::new_token();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at, machine_name) \
         VALUES ($1, $2, now() + ($3 || ' hours')::interval, $4)",
    )
    .bind(&digest)
    .bind(user_id)
    .bind(auth::SESSION_TTL_HOURS.to_string())
    .bind(&req.machine_name)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({ "token": raw, "user_id": user_id })))
}

/// Who the presented token belongs to.
///
/// The login response already carries this, but a client that restarts holds
/// only a token. Without this endpoint it cannot tell whether a lock is its own
/// or a colleague's — and "you are already in this profile" versus "take it away
/// from someone" must never be the same button.
async fn whoami(
    State(state): State<Arc<AppState>>,
    caller: Caller,
) -> ApiResult<Json<serde_json::Value>> {
    let (email,): (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(caller.user_id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({
        "user_id": caller.user_id,
        "email": email,
        "org_id": caller.org_id,
        "role": caller.role,
    })))
}

async fn logout(State(state): State<Arc<AppState>>, caller: Caller) -> ApiResult<Json<serde_json::Value>> {
    // Every session of this user, not just the presented one: "log me out" from
    // someone who thinks they were compromised has to mean everywhere.
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(caller.user_id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// projects
// ---------------------------------------------------------------------------

async fn list_projects(
    State(state): State<Arc<AppState>>,
    caller: Caller,
) -> ApiResult<Json<Vec<ProjectSummary>>> {
    // Resolved in SQL rather than by filtering in Rust: a project the caller
    // cannot see must never be loaded, so that a bug in the mapping code cannot
    // leak one.
    let rows: Vec<(Uuid, String, String, Option<String>, i64, Option<i64>)> = sqlx::query_as(
        r#"
        SELECT p.id, p.name, p.description, p.color,
               (SELECT count(*) FROM profiles f
                 WHERE f.project_id = p.id AND f.deleted_at IS NULL),
               g.permissions
        FROM projects p
        JOIN org_members m ON m.org_id = p.org_id AND m.user_id = $1
        LEFT JOIN project_grants g
               ON g.project_id = p.id AND g.user_id = $1
              AND (g.expires_at IS NULL OR g.expires_at > now())
        WHERE p.deleted_at IS NULL
          AND ($2 OR g.permissions IS NOT NULL)
        ORDER BY p.name
        "#,
    )
    .bind(caller.user_id)
    .bind(fury_shared::rbac::has_implicit_project_access(caller.role))
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, name, description, color, profile_count, grant)| ProjectSummary {
                id,
                name,
                description,
                color,
                profile_count,
                permissions: effective(caller.role, grant.map(PermSet)).to_vec(),
            })
            .collect(),
    ))
}

async fn list_profiles(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ProfileSummary>>> {
    let perms = rbac_guard::permissions_for(&state.db, caller.user_id, project_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::View) {
        return Err(ApiError::Denied(Perm::View));
    }

    let reveal = perms.has(Perm::RevealSecrets);

    let rows: Vec<(
        Uuid, String, Vec<String>, String, i32,
        Option<Uuid>, Option<String>, Option<String>, Option<String>, Option<String>,
        Option<Uuid>, Option<String>, Option<String>, Option<String>, Option<String>,
    )> = sqlx::query_as(
        &format!(
            r#"
        SELECT f.id, f.name, f.tags, f.persona_id, f.current_version,
               x.id, x.name, x.kind::text, x.host, x.last_country,
               l.user_id, u.email, l.machine_name,
               {acquired}, {expires}
        FROM profiles f
        LEFT JOIN proxies x ON x.id = f.proxy_id AND x.deleted_at IS NULL
        LEFT JOIN profile_locks l ON l.profile_id = f.id AND l.expires_at > now()
        LEFT JOIN users u ON u.id = l.user_id
        WHERE f.project_id = $1 AND f.deleted_at IS NULL
        ORDER BY f.name
        "#,
            acquired = rfc3339("l.acquired_at"),
            expires = rfc3339("l.expires_at"),
        ),
    )
    .bind(project_id)
    .fetch_all(&state.db)
    .await?;

    let out = rows
        .into_iter()
        .map(
            |(
                id, name, tags, persona_id, current_version,
                proxy_id, proxy_name, proxy_kind, proxy_host, proxy_country,
                lock_user, lock_email, lock_machine, lock_acquired, lock_expires,
            )| {
                ProfileSummary {
                    id,
                    project_id,
                    name,
                    tags,
                    persona_id,
                    current_version,
                    proxy: proxy_id.map(|pid| ProxySummary {
                        id: pid,
                        name: proxy_name.unwrap_or_default(),
                        kind: proxy_kind.unwrap_or_default(),
                        // Without reveal_secrets the host is masked here, on the
                        // server. Masking in the UI would still have sent the
                        // real value over the wire.
                        display: match (&proxy_host, reveal) {
                            (Some(h), true) => h.clone(),
                            (Some(h), false) => mask_host(h),
                            (None, _) => String::new(),
                        },
                        country: proxy_country,
                        detected_kind: None,
                        last_checked_at: None,
                        shared_with_profiles: 0,
                    }),
                    lock: lock_user.map(|uid| LockInfo {
                        user_id: uid,
                        user_email: lock_email.unwrap_or_default(),
                        machine_name: lock_machine.unwrap_or_default(),
                        acquired_at: lock_acquired.unwrap_or_default(),
                        expires_at: lock_expires.unwrap_or_default(),
                    }),
                    permissions: perms.to_vec(),
                }
            },
        )
        .collect();

    Ok(Json(out))
}

/// SQL that renders a `timestamptz` as RFC 3339 in UTC.
///
/// Every timestamp crossing the wire goes through this. The first version used
/// `to_char(..., 'OF')`, which writes a whole-hour offset as `+00` — legal in
/// Postgres, not legal in RFC 3339, and `new Date()` in the UI answered
/// `Invalid Date`. Converting to UTC and appending a literal `Z` removes the
/// variability instead of trying to spell the offset correctly: there is one
/// possible output shape, and it parses everywhere.
///
/// The argument is always a column reference written in this file, never
/// anything derived from a request.
fn rfc3339(column: &str) -> String {
    format!(r#"to_char({column} AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"')"#)
}

/// Keeps enough of a hostname to tell two proxies apart, not enough to reuse
/// one. An operator must be able to see which exit a profile uses without
/// walking away with the organisation's proxy list.
///
/// The trailing hash is what makes this work, and its absence is what the first
/// version got wrong: truncating to a prefix mapped `res-eu-01.provider.net`
/// and `res-us-02.provider.net` — the ordinary case, two exits from one
/// provider — onto the same string, so the operator could not tell which proxy
/// a profile used. A short digest of the whole host distinguishes reliably and
/// discloses nothing.
fn mask_host(host: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(host.as_bytes());
    let tag = format!("{:02x}{:02x}", digest[0], digest[1]);
    match host.split_once('.') {
        // Only show a prefix when the label is long enough that hiding the rest
        // still hides something. For "ab" a three-character prefix is the whole
        // label, so nothing is masked at all.
        Some((first, rest)) if first.chars().count() > 4 => {
            let head: String = first.chars().take(3).collect();
            format!("{head}***{tag}.{rest}")
        }
        Some((_, rest)) => format!("***{tag}.{rest}"),
        None => format!("***{tag}"),
    }
}

async fn grant_access(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(project_id): Path<Uuid>,
    Json(req): Json<GrantRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    rbac_guard::require(&state.db, caller.user_id, project_id, Perm::ManageAccess).await?;

    // The grant is capped by the recipient's org role, not the granter's. A
    // manager cannot promote a member past what a member may ever hold — see
    // role_ceiling in shared-rs.
    let target_role: Option<(String,)> = sqlx::query_as(
        "SELECT m.role::text FROM org_members m \
         JOIN projects p ON p.org_id = m.org_id \
         WHERE m.user_id = $1 AND p.id = $2",
    )
    .bind(req.user_id)
    .bind(project_id)
    .fetch_optional(&state.db)
    .await?;

    let Some((role,)) = target_role else {
        return Err(ApiError::BadRequest(
            "that user is not a member of this organisation".into(),
        ));
    };
    let role = auth::parse_role(&role).ok_or(ApiError::NotFound)?;
    let requested = PermSet::from_iter(req.permissions.iter().copied());
    let capped = requested.intersect(fury_shared::rbac::role_ceiling(role));

    sqlx::query(
        "INSERT INTO project_grants (project_id, user_id, permissions, granted_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (project_id, user_id) DO UPDATE \
         SET permissions = EXCLUDED.permissions, granted_by = EXCLUDED.granted_by, \
             granted_at = now(), expires_at = EXCLUDED.expires_at",
    )
    .bind(project_id)
    .bind(req.user_id)
    .bind(capped.0)
    .bind(caller.user_id)
    .bind(req.expires_at.as_deref())
    .execute(&state.db)
    .await?;

    audit(&state, &caller, "access.grant", Some(project_id),
          json!({ "target": req.user_id, "requested": requested.to_vec(), "granted": capped.to_vec() }))
        .await?;

    Ok(Json(json!({ "granted": capped.to_vec() })))
}

// ---------------------------------------------------------------------------
// locks
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LockRequest {
    machine_id: String,
    machine_name: String,
    /// Only honoured with manage_access, and always audited.
    #[serde(default)]
    force: bool,
}

async fn acquire_lock(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(profile_id): Path<Uuid>,
    Json(req): Json<LockRequest>,
) -> ApiResult<Json<AcquireLockResponse>> {
    let perms = rbac_guard::permissions_for_profile(&state.db, caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::Launch) {
        return Err(ApiError::Denied(Perm::Launch));
    }
    if req.force && !perms.has(Perm::ManageAccess) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    let (raw, digest) = auth::new_token();

    // One statement, so two agents racing cannot both win.
    //
    // ON CONFLICT DO UPDATE with a WHERE, not a CTE that deletes first: a
    // DELETE and an INSERT in the same statement see the same snapshot, so the
    // INSERT still conflicts with the row the DELETE just removed. Measured —
    // the first version made an authorised force-unlock return 409, which is
    // precisely the case it existed to serve.
    //
    // The WHERE decides who may take over: an expired lock is free for anyone
    // with launch, a live one only yields to an authorised force.
    let inserted: Option<(i32,)> = sqlx::query_as(
        r#"
        INSERT INTO profile_locks
            (profile_id, user_id, token_hash, machine_name, machine_id, expires_at)
        VALUES ($1, $2, $3, $4, $6, now() + interval '90 seconds')
        ON CONFLICT (profile_id) DO UPDATE
        SET user_id = EXCLUDED.user_id,
            token_hash = EXCLUDED.token_hash,
            machine_name = EXCLUDED.machine_name,
            machine_id = EXCLUDED.machine_id,
            acquired_at = now(),
            expires_at = EXCLUDED.expires_at
        WHERE profile_locks.expires_at <= now() OR $5
        RETURNING 1
        "#,
    )
    .bind(profile_id)
    .bind(caller.user_id)
    .bind(&digest)
    .bind(&req.machine_name)
    .bind(req.force)
    .bind(&req.machine_id)
    .fetch_optional(&state.db)
    .await?;

    if inserted.is_none() {
        // Someone else holds it. Say who — the operator needs to know which
        // colleague to ask, and "locked" without a name is what turns this into
        // a chat message.
        let holder: Option<(String, String)> = sqlx::query_as(
            "SELECT u.email, l.machine_name FROM profile_locks l \
             JOIN users u ON u.id = l.user_id WHERE l.profile_id = $1",
        )
        .bind(profile_id)
        .fetch_optional(&state.db)
        .await?;
        let (email, machine) = holder.unwrap_or_default();
        return Err(ApiError::Locked(
            json!({ "user_email": email, "machine_name": machine }),
        ));
    }

    if req.force {
        audit(&state, &caller, "lock.force", Some(profile_id), json!({})).await?;
    }
    audit(&state, &caller, "profile.launch", Some(profile_id),
          json!({ "machine": req.machine_name })).await?;

    // The launching agent is told exactly how to harden the browser. Computed
    // here rather than trusted from the client, because these flags are the
    // technical half of "an operator cannot take the data home".
    let restrictions = LaunchRestrictions::for_perms(perms);

    // Reported back so the agent knows when its heartbeat must land.
    let expires_at: (String,) = sqlx::query_as(&format!(
        "SELECT {} FROM profile_locks WHERE profile_id = $1",
        rfc3339("expires_at")
    ))
    .bind(profile_id)
    .fetch_one(&state.db)
    .await?;
    let expires_at = expires_at.0;

    Ok(Json(AcquireLockResponse {
        lock_token: raw,
        expires_at: expires_at.to_string(),
        restrictions,
    }))
}

async fn heartbeat(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(profile_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    let token = body
        .get("lock_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("lock_token is required".into()))?;

    // Matched on the token, not just the user: an agent whose lock was
    // force-released must not be able to extend it back into existence.
    let updated = sqlx::query(
        "UPDATE profile_locks SET expires_at = now() + interval '90 seconds' \
         WHERE profile_id = $1 AND user_id = $2 AND token_hash = $3",
    )
    .bind(profile_id)
    .bind(caller.user_id)
    .bind(auth::hash_token(token))
    .execute(&state.db)
    .await?;

    if updated.rows_affected() == 0 {
        return Err(ApiError::Conflict(
            "this lock is no longer yours; stop and re-acquire before uploading".into(),
        ));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn release_lock(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(profile_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("DELETE FROM profile_locks WHERE profile_id = $1 AND user_id = $2")
        .bind(profile_id)
        .bind(caller.user_id)
        .execute(&state.db)
        .await?;
    audit(&state, &caller, "profile.stop", Some(profile_id), json!({})).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------

/// Appends to the audit log. Failures are logged, never propagated: losing an
/// audit row must not fail the operation the user asked for, and the row is
/// append-only by grant, so it cannot be rewritten afterwards either.
async fn audit(
    state: &AppState,
    caller: &Caller,
    action: &str,
    target: Option<Uuid>,
    detail: serde_json::Value,
) -> ApiResult<()> {
    if let Err(e) = sqlx::query(
        "INSERT INTO audit_events (org_id, actor_user_id, action, target_id, detail) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(caller.org_id)
    .bind(caller.user_id)
    .bind(action)
    .bind(target)
    .bind(detail)
    .execute(&state.db)
    .await
    {
        tracing::error!(error = %e, action, "audit write failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_hosts_stay_distinguishable_but_unusable() {
        // An operator must be able to tell two proxies apart without being able
        // to reuse either.
        let a = mask_host("res-eu-01.provider.net");
        let b = mask_host("res-us-02.provider.net");
        // Two exits from one provider is the ordinary case, and the first
        // version of this collapsed them onto the same string.
        assert_ne!(a, b);
        assert!(!a.contains("eu-01"));
        assert!(!b.contains("us-02"));
        assert!(a.ends_with(".provider.net"));
        // Stable, or the list would appear to change on every refresh.
        assert_eq!(a, mask_host("res-eu-01.provider.net"));
    }

    #[test]
    fn masking_handles_hosts_without_a_dot() {
        assert!(mask_host("localhost").starts_with("***"));
        assert!(mask_host("ab.example.com").ends_with(".example.com"));
        assert!(!mask_host("ab.example.com").starts_with("ab***"));
    }
}
