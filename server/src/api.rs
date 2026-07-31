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
        .route("/v1/auth/enroll", post(complete_enrollment))
        .route("/v1/auth/enroll/{code}", get(peek_enrollment))
        .route("/v1/me", get(whoami))
        .route("/v1/projects", get(list_projects).post(create_project))
        .route(
            "/v1/projects/{project_id}",
            axum::routing::patch(rename_project).delete(delete_project),
        )
        .route("/v1/projects/{project_id}/profiles", get(list_profiles))
        .route(
            "/v1/projects/{project_id}/grants",
            get(list_grants).post(grant_access),
        )
        .route(
            "/v1/projects/{project_id}/grants/{user_id}",
            axum::routing::delete(revoke_access),
        )
        .route("/v1/org/members", get(list_members))
        .route("/v1/org/invitations", post(create_invitation))
        .route("/v1/org/members/{user_id}/key", post(hand_over_key))
        .route("/v1/profiles/{profile_id}/lock", post(acquire_lock))
        .route("/v1/profiles/{profile_id}/lock/heartbeat", post(heartbeat))
        .route("/v1/profiles/{profile_id}/unlock", post(release_lock))
        .route(
            "/v1/profiles/{profile_id}/bundle",
            post(upload_bundle).get(download_bundle),
        )
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
    let row: Option<(Uuid, String, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT id, password_hash, wrapped_private_key, kdf_salt \
         FROM users WHERE email = $1 AND disabled_at IS NULL",
    )
    .bind(&req.email)
    .fetch_optional(&state.db)
    .await?;

    // One failure mode for "no such user" and "wrong password", or the endpoint
    // becomes a way to discover who has an account.
    let Some((user_id, stored, wrapped_private_key, kdf_salt)) = row else {
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

    // The wrapped keys travel with the session, because the client needs them
    // on every machine and holding them locally would mean a second place they
    // can be lost. They are useless to anyone who intercepts them: the private
    // key opens only under a key derived from the password, and the ORK opens
    // only under the private key.
    let org: Option<(Uuid, Option<Vec<u8>>, String)> = sqlx::query_as(
        "SELECT org_id, wrapped_ork, role::text FROM org_members WHERE user_id = $1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;

    use base64::Engine;
    let b64 = |v: &[u8]| base64::engine::general_purpose::STANDARD.encode(v);

    Ok(Json(json!({
        "token": raw,
        "user_id": user_id,
        "wrapped_private_key": b64(&wrapped_private_key),
        "kdf_salt": b64(&kdf_salt),
        "org_id": org.as_ref().map(|(id, _, _)| *id),
        "role": org.as_ref().map(|(_, _, r)| r.clone()),
        // None means enrolled but not yet handed the organisation key: they can
        // sign in and see the shape of the team, and decrypt nothing.
        "wrapped_ork": org.as_ref().and_then(|(_, ork, _)| ork.as_deref()).map(b64),
    })))
}

// ---------------------------------------------------------------------------
// enrolment
// ---------------------------------------------------------------------------

/// An invitation as stored. `org_id` and `org_name` are exclusive: one joins an
/// existing organisation, the other creates it — see the check constraint in
/// migration 0003.
#[derive(sqlx::FromRow)]
struct Invitation {
    id: Uuid,
    email: String,
    org_name: Option<String>,
    org_id: Option<Uuid>,
    role: String,
}

/// What an invitation is for, before anyone commits to it.
///
/// Shown so the person typing a code sees "you are joining My team as owner"
/// rather than a password field and a hope. It reveals the invited address to
/// whoever holds the code — who already holds the code, and could simply use
/// it.
async fn peek_enrollment(
    State(state): State<Arc<AppState>>,
    Path(code): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let row: Option<Invitation> = sqlx::query_as(
        "SELECT id, email, org_name, org_id, role::text AS role \
         FROM enrollments \
         WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(crate::enroll::hash_code(&code))
    .fetch_optional(&state.db)
    .await?;

    // Expired, spent and never-existed are one answer. Distinguishing them
    // turns this endpoint into a way to test codes.
    let Some(inv) = row else {
        return Err(ApiError::NotFound);
    };
    let Invitation { email, org_name, org_id, role, .. } = inv;

    let org = match (org_name, org_id) {
        (Some(name), _) => name,
        (None, Some(id)) => {
            sqlx::query_scalar::<_, String>("SELECT name FROM organizations WHERE id = $1")
                .bind(id)
                .fetch_one(&state.db)
                .await?
        }
        (None, None) => return Err(ApiError::NotFound),
    };

    Ok(Json(json!({
        "email": email,
        "organization": org,
        "role": role,
        // An owner brings the organisation key into being; everyone else waits
        // to be handed it. The client needs to know which job it has.
        "creates_org_key": role == "owner",
    })))
}

/// Everything the client generated, in one request.
///
/// The server checks the code and stores the material. It cannot read any of
/// it: `wrapped_private_key` is sealed under a key derived from the password on
/// the client, and `wrapped_ork` is sealed to the public key. What arrives here
/// as `password` is used for one thing — the Argon2id login hash — and is not
/// the input to either wrapping, which uses a separate salt the client keeps.
#[derive(Deserialize)]
pub struct EnrollRequest {
    code: String,
    password: String,
    /// base64, all of them. The server never parses these beyond decoding.
    public_key: String,
    wrapped_private_key: String,
    kdf_salt: String,
    /// Present when the invitation creates the organisation, absent otherwise.
    #[serde(default)]
    wrapped_ork: Option<String>,
    #[serde(default)]
    machine_name: String,
}

async fn complete_enrollment(
    State(state): State<Arc<AppState>>,
    Json(req): Json<EnrollRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    use base64::Engine;
    let b64 = |what: &str, v: &str| -> Result<Vec<u8>, ApiError> {
        base64::engine::general_purpose::STANDARD
            .decode(v)
            .map_err(|_| ApiError::BadRequest(format!("{what} is not base64")))
    };

    let public_key = b64("public_key", &req.public_key)?;
    let wrapped_private_key = b64("wrapped_private_key", &req.wrapped_private_key)?;
    let kdf_salt = b64("kdf_salt", &req.kdf_salt)?;
    let wrapped_ork = req
        .wrapped_ork
        .as_deref()
        .map(|v| b64("wrapped_ork", v))
        .transpose()?;

    // X25519. A key of the wrong length would be stored happily and fail years
    // later, when someone tries to wrap the ORK to it.
    if public_key.len() != 32 {
        return Err(ApiError::BadRequest(
            "public_key must be 32 bytes — X25519".into(),
        ));
    }
    if req.password.len() < 12 {
        return Err(ApiError::BadRequest(
            "the password must be at least 12 characters: it is the only thing standing \
             between a stolen laptop and this organisation's accounts"
                .into(),
        ));
    }

    // One transaction from here: a half-enrolled owner is an organisation
    // nobody can enter and the code cannot be spent again to fix it.
    let mut tx = state.db.begin().await?;

    let row: Option<Invitation> = sqlx::query_as(
        "SELECT id, email, org_name, org_id, role::text AS role FROM enrollments \
         WHERE code_hash = $1 AND used_at IS NULL AND expires_at > now() \
         FOR UPDATE",
    )
    .bind(crate::enroll::hash_code(&req.code))
    .fetch_optional(&mut *tx)
    .await?;

    let Some(Invitation { id: enrollment_id, email, org_name, org_id, role }) = row else {
        return Err(ApiError::BadRequest(
            "this invitation is not valid — it may have been used already, or expired".into(),
        ));
    };

    let owner = role == "owner";
    if owner && wrapped_ork.is_none() {
        return Err(ApiError::BadRequest(
            "an owner enrolment must carry the wrapped organisation key".into(),
        ));
    }

    // An organisation is created at the moment it acquires an owner, never
    // before: a bootstrap abandoned halfway leaves nothing behind.
    let org_id = match (org_id, org_name) {
        (Some(id), _) => id,
        (None, Some(name)) => {
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
                .bind(id)
                .bind(&name)
                .execute(&mut *tx)
                .await?;
            id
        }
        (None, None) => return Err(ApiError::Internal(anyhow::anyhow!("enrolment targets no org"))),
    };

    let user_id = Uuid::now_v7();
    let password_hash = auth::hash_password(&req.password)
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("{e}")))?;

    sqlx::query(
        "INSERT INTO users (id, email, password_hash, public_key, wrapped_private_key, kdf_salt) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(user_id)
    .bind(&email)
    .bind(&password_hash)
    .bind(&public_key)
    .bind(&wrapped_private_key)
    .bind(&kdf_salt)
    .execute(&mut *tx)
    .await
    .map_err(|e| match &e {
        // The address is taken. Saying so is not a leak: whoever holds this
        // invitation was told the address it was issued for.
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            ApiError::BadRequest(format!("{email} already has an account on this server"))
        }
        _ => ApiError::Db(e),
    })?;

    sqlx::query(
        "INSERT INTO org_members (org_id, user_id, role, wrapped_ork, ork_generation) \
         VALUES ($1, $2, $3::org_role, $4, $5)",
    )
    .bind(org_id)
    .bind(user_id)
    .bind(&role)
    .bind(&wrapped_ork)
    // A member with no key yet has no generation either — it is set when the
    // key is handed over, and must record which generation was handed.
    .bind(wrapped_ork.as_ref().map(|_| 1i32))
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE enrollments SET used_at = now() WHERE id = $1")
        .bind(enrollment_id)
        .execute(&mut *tx)
        .await?;

    // Signed in on the way out. Asking someone to type the password they just
    // chose, into the same window, teaches them nothing.
    let (raw, digest) = auth::new_token();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at, machine_name) \
         VALUES ($1, $2, now() + ($3 || ' hours')::interval, $4)",
    )
    .bind(&digest)
    .bind(user_id)
    .bind(auth::SESSION_TTL_HOURS.to_string())
    .bind(&req.machine_name)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    tracing::info!(%user_id, %org_id, %role, "enrolled");
    Ok(Json(json!({
        "token": raw,
        "user_id": user_id,
        "org_id": org_id,
        "role": role,
        "has_org_key": wrapped_ork.is_some(),
    })))
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

#[derive(Deserialize)]
pub struct ProjectRequest {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    color: Option<String>,
}

/// Make a project.
///
/// Owners and admins only. A project is the unit access is granted over, so the
/// right to create one is the right to create a container whose membership you
/// then decide — which is an administrative act, not an operator's.
async fn create_project(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Json(req): Json<ProjectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("a project needs a name".into()));
    }

    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO projects (id, org_id, name, description, color, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(caller.org_id)
    .bind(name)
    .bind(&req.description)
    .bind(&req.color)
    .bind(caller.user_id)
    .execute(&state.db)
    .await?;

    audit(&state, &caller, "project.create", Some(id), json!({ "name": name })).await?;
    Ok(Json(json!({ "id": id })))
}

async fn rename_project(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(project_id): Path<Uuid>,
    Json(req): Json<ProjectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    rbac_guard::require(&state.db, caller.user_id, project_id, Perm::ManageAccess).await?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("a project needs a name".into()));
    }

    sqlx::query(
        "UPDATE projects SET name = $1, description = $2, color = $3 \
         WHERE id = $4 AND org_id = $5 AND deleted_at IS NULL",
    )
    .bind(name)
    .bind(&req.description)
    .bind(&req.color)
    .bind(project_id)
    .bind(caller.org_id)
    .execute(&state.db)
    .await?;

    audit(&state, &caller, "project.rename", Some(project_id), json!({ "name": name })).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Soft-delete, with its profiles.
///
/// The same rule the local store follows: a project that disappears while its
/// profiles stay visible somewhere else is a lie, and a project whose deletion
/// destroys a warmed account outright is a disaster. Both go to the same
/// deleted state, in one transaction, and both come back together.
///
/// Refused while anything inside is open. A profile whose browser is running
/// will push a bundle when it closes, and pushing into a deleted project is a
/// write nobody can find afterwards.
async fn delete_project(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    let mut tx = state.db.begin().await?;

    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM profile_locks l JOIN profiles p ON p.id = l.profile_id \
         WHERE p.project_id = $1 AND l.expires_at > now()",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    if open > 0 {
        return Err(ApiError::Conflict(format!(
            "{open} profile(s) in this project are open right now. Close them first"
        )));
    }

    let profiles: i64 = sqlx::query_scalar(
        "UPDATE profiles SET deleted_at = now() \
         WHERE project_id = $1 AND deleted_at IS NULL RETURNING 1",
    )
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await
    .map(|r: Vec<i64>| r.len() as i64)?;

    let updated = sqlx::query(
        "UPDATE projects SET deleted_at = now() \
         WHERE id = $1 AND org_id = $2 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .bind(caller.org_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    tx.commit().await?;
    audit(&state, &caller, "project.delete", Some(project_id), json!({ "profiles": profiles })).await?;
    Ok(Json(json!({ "deleted": true, "profiles": profiles })))
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

    // $5::timestamptz, not $5. The expiry arrives as an RFC 3339 string — the
    // same shape this API hands out — and Postgres will not coerce text into a
    // timestamptz in a parameter position. Without the cast every grant failed
    // with a 500, which is to say access could not be given to anybody at all.
    sqlx::query(
        "INSERT INTO project_grants (project_id, user_id, permissions, granted_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5::timestamptz) \
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

/// Who can reach this project, and with what.
///
/// Owners and admins reach every project implicitly and have no grant row, so
/// they are listed separately rather than omitted — a screen that showed only
/// the rows would read as "nobody else can see this", which is false.
async fn list_grants(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    rbac_guard::require(&state.db, caller.user_id, project_id, Perm::ManageAccess).await?;

    let rows: Vec<(Uuid, String, String, i64, Option<String>)> = sqlx::query_as(
        &format!(
        "SELECT u.id, u.email, m.role::text, g.permissions, {} \
         FROM project_grants g \
         JOIN users u ON u.id = g.user_id \
         JOIN projects p ON p.id = g.project_id \
         JOIN org_members m ON m.user_id = u.id AND m.org_id = p.org_id \
         WHERE g.project_id = $1 \
         ORDER BY u.email",
        rfc3339("g.expires_at"),
    ))
    .bind(project_id)
    .fetch_all(&state.db)
    .await?;

    let implicit: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT u.id, u.email, m.role::text \
         FROM org_members m \
         JOIN users u ON u.id = m.user_id \
         JOIN projects p ON p.org_id = m.org_id \
         WHERE p.id = $1 AND m.role IN ('owner', 'admin') \
         ORDER BY u.email",
    )
    .bind(project_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({
        "granted": rows.iter().map(|(id, email, role, perms, expires)| json!({
            "user_id": id,
            "email": email,
            "role": role,
            "permissions": PermSet(*perms).to_vec(),
            "expires_at": expires,
        })).collect::<Vec<_>>(),
        "implicit": implicit.iter().map(|(id, email, role)| json!({
            "user_id": id, "email": email, "role": role,
        })).collect::<Vec<_>>(),
    })))
}

/// Take a project away from someone.
///
/// Deleting the grant is immediate and total: permissions are resolved per
/// request, never cached in a token, which is the reason sessions are opaque
/// and revocable in the first place (auth.rs). What it does not do is reach
/// into bundles they already downloaded — the honest boundary of any system
/// where work happens on the operator's own machine.
async fn revoke_access(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path((project_id, user_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    rbac_guard::require(&state.db, caller.user_id, project_id, Perm::ManageAccess).await?;

    sqlx::query("DELETE FROM project_grants WHERE project_id = $1 AND user_id = $2")
        .bind(project_id)
        .bind(user_id)
        .execute(&state.db)
        .await?;

    // Any lock they hold goes too. Otherwise a revoked operator's browser keeps
    // a profile marked in-use for as long as its heartbeat runs, and the team
    // cannot open a profile they no longer have any right to.
    sqlx::query(
        "DELETE FROM profile_locks l USING profiles p \
         WHERE l.profile_id = p.id AND p.project_id = $1 AND l.user_id = $2",
    )
    .bind(project_id)
    .bind(user_id)
    .execute(&state.db)
    .await?;

    audit(&state, &caller, "access.revoke", Some(project_id), json!({ "target": user_id })).await?;
    Ok(Json(json!({ "revoked": true })))
}

// ---------------------------------------------------------------------------
// the organisation
// ---------------------------------------------------------------------------

/// Everyone in the organisation, and whether they can actually decrypt anything.
///
/// `has_key` is the distinction that matters and the one nothing else surfaces:
/// enrolling makes an account, but until someone wraps the organisation key to
/// that person's public key they can sign in, see the team, and open nothing.
/// `public_key` is here because wrapping it is the client's job — the server
/// has never held the key it would need.
async fn list_members(
    State(state): State<Arc<AppState>>,
    caller: Caller,
) -> ApiResult<Json<serde_json::Value>> {
    let rows: Vec<(Uuid, String, String, Vec<u8>, Option<Vec<u8>>, String)> =
        sqlx::query_as(&format!(
            "SELECT u.id, u.email, m.role::text, u.public_key, m.wrapped_ork, {} \
             FROM org_members m JOIN users u ON u.id = m.user_id \
             WHERE m.org_id = $1 AND u.disabled_at IS NULL \
             ORDER BY m.joined_at",
            rfc3339("m.joined_at"),
        ))
        .bind(caller.org_id)
        .fetch_all(&state.db)
        .await?;

    use base64::Engine;
    let b64 = |v: &[u8]| base64::engine::general_purpose::STANDARD.encode(v);

    // Pending invitations are part of the picture: an owner who cannot see that
    // they already invited someone issues a second code and wonders why.
    let pending: Vec<(String, String, String)> = sqlx::query_as(&format!(
        "SELECT email, role::text, {} FROM enrollments \
         WHERE org_id = $1 AND used_at IS NULL AND expires_at > now() ORDER BY created_at",
        rfc3339("expires_at"),
    ))
    .bind(caller.org_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(json!({
        "members": rows.iter().map(|(id, email, role, pk, ork, joined)| json!({
            "user_id": id,
            "email": email,
            "role": role,
            "public_key": b64(pk),
            "has_key": ork.is_some(),
            "joined_at": joined,
            "is_you": *id == caller.user_id,
        })).collect::<Vec<_>>(),
        "invited": pending.iter().map(|(email, role, expires)| json!({
            "email": email, "role": role, "expires_at": expires,
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct InviteRequest {
    email: String,
    role: String,
}

/// Invite someone, from the application rather than from a shell on the server.
///
/// The command-line form still exists and is the only way to create the first
/// owner — there is nobody to invite them. Everything after that happens here,
/// because asking an operator to ssh into a box to add a colleague is asking
/// them not to.
async fn create_invitation(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Json(req): Json<InviteRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }
    let role = auth::parse_role(&req.role)
        .ok_or_else(|| ApiError::BadRequest("unknown role".into()))?;
    // One owner per organisation: the key hierarchy has no story for two, and a
    // second would be a person nobody can remove.
    if matches!(role, OrgRole::Owner) {
        return Err(ApiError::BadRequest(
            "an organisation has one owner. Invite as admin for the same reach".into(),
        ));
    }
    // An admin cannot mint another admin: it would let anyone who reaches one
    // admin account grant themselves a second, and removing the first would no
    // longer remove the access.
    if matches!(caller.role, OrgRole::Admin) && matches!(role, OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    let email = req.email.trim();
    if !email.contains('@') || email.len() < 3 {
        return Err(ApiError::BadRequest("that is not an email address".into()));
    }

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users u JOIN org_members m ON m.user_id = u.id \
         WHERE u.email = $1 AND m.org_id = $2)",
    )
    .bind(email)
    .bind(caller.org_id)
    .fetch_one(&state.db)
    .await?;
    if exists {
        return Err(ApiError::BadRequest(format!("{email} is already in this team")));
    }

    let code = crate::enroll::issue(
        &state.db,
        email,
        crate::enroll::Org::Existing(caller.org_id),
        &req.role,
    )
    .await
    .map_err(ApiError::Internal)?;

    audit(&state, &caller, "member.invite", None, json!({ "email": email, "role": req.role })).await?;

    // Shown once. The row holds a hash, so this is the only time it exists.
    Ok(Json(json!({
        "code": code,
        "expires_in_hours": crate::enroll::TTL_HOURS,
    })))
}

#[derive(Deserialize)]
pub struct HandOverRequest {
    /// The organisation key, sealed to this member's public key by a client
    /// that holds it. base64.
    wrapped_ork: String,
}

/// Let a member in.
///
/// The second half of an invitation, and the half that cannot happen on the
/// server: the organisation key is sealed to the member's public key by someone
/// who already holds it, on their own machine. Until this runs, the member has
/// an account and no ability to decrypt a single profile.
async fn hand_over_key(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(user_id): Path<Uuid>,
    Json(req): Json<HandOverRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    use base64::Engine;
    let wrapped = base64::engine::general_purpose::STANDARD
        .decode(&req.wrapped_ork)
        .map_err(|_| ApiError::BadRequest("wrapped_ork is not base64".into()))?;
    // 32 ephemeral public key + 24 nonce + 32 payload + 16 tag. Shorter cannot
    // be a sealed key, and storing one would fail on the member's machine at
    // sign-in with nothing to point at.
    if wrapped.len() < 104 {
        return Err(ApiError::BadRequest(
            "that is not a sealed organisation key".into(),
        ));
    }

    // Scoped to the caller's own organisation, and to the generation in force.
    // Without the org clause an admin of one team could write a key into
    // another team's membership row.
    let generation: i32 = sqlx::query_scalar("SELECT ork_generation FROM organizations WHERE id = $1")
        .bind(caller.org_id)
        .fetch_one(&state.db)
        .await?;

    let updated = sqlx::query(
        "UPDATE org_members SET wrapped_ork = $1, ork_generation = $2 \
         WHERE org_id = $3 AND user_id = $4",
    )
    .bind(&wrapped)
    .bind(generation)
    .bind(caller.org_id)
    .bind(user_id)
    .execute(&state.db)
    .await?;

    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    audit(&state, &caller, "member.key", None, json!({ "target": user_id })).await?;
    Ok(Json(json!({ "ok": true })))
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

// ---------------------------------------------------------------------------
// bundles
// ---------------------------------------------------------------------------
//
// Bytes go through the server for now. docs/01 says it should not proxy them,
// and for a public host that is right — presigned URLs into object storage, so
// the server stays small and the bandwidth bill lands where the storage is. For
// one team on one box, a directory is the honest answer: it works, it backs up
// with the rest of the machine, and it does not require standing up MinIO to
// share a profile with three colleagues.
//
// What the server never has is a key. It stores ciphertext, the wrapped key it
// cannot open, and a digest *of the ciphertext* so it can verify what it holds
// without being able to read it.

pub fn bundle_root() -> std::path::PathBuf {
    std::env::var("FURY_BUNDLE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/var/lib/fury/bundles"))
}

/// Checked once, at startup.
///
/// The alternative is finding out at the first upload, which is the first time
/// someone closes a browser after a full working session — the worst possible
/// moment to discover that the directory is missing or read-only, because the
/// answer arrives as a 500 and the operator's next question is where their
/// afternoon went.
pub fn check_bundle_root() -> anyhow::Result<std::path::PathBuf> {
    let dir = bundle_root();
    std::fs::create_dir_all(&dir).map_err(|e| {
        anyhow::anyhow!(
            "cannot create the bundle directory {} ({e}). Create it and give this process \
             ownership, or set FURY_BUNDLE_DIR somewhere writable",
            dir.display()
        )
    })?;

    let probe = dir.join(".writable");
    std::fs::write(&probe, b"").map_err(|e| {
        anyhow::anyhow!(
            "the bundle directory {} exists but is not writable ({e})",
            dir.display()
        )
    })?;
    let _ = std::fs::remove_file(&probe);
    Ok(dir)
}

/// Upload a new version.
///
/// The version the client based its work on comes in a header, and a mismatch
/// is a 409 rather than an overwrite. That check is the whole point: two people
/// with the same profile open produce two bundles, and silently keeping the
/// later one loses whatever the first did — which for a warmed account can be
/// weeks of work.
async fn upload_bundle(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(profile_id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let perms = rbac_guard::permissions_for_profile(&state.db, caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    // Uploading is what launching a profile leads to, so it is gated by the
    // same right rather than one of its own: anyone who may open a profile is
    // necessarily allowed to change it, and a separate permission would only
    // produce operators who can work but cannot save.
    if !perms.has(Perm::Launch) {
        return Err(ApiError::Denied(Perm::Launch));
    }

    let header = |name: &str| -> Option<String> {
        headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
    };

    let wrapped_key = header("x-fury-wrapped-key")
        .ok_or_else(|| ApiError::BadRequest("missing x-fury-wrapped-key".into()))?;
    let sha256 = header("x-fury-sha256")
        .ok_or_else(|| ApiError::BadRequest("missing x-fury-sha256".into()))?;
    let base_version: i32 = header("x-fury-base-version")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| ApiError::BadRequest("missing x-fury-base-version".into()))?;

    // Holding the lock is what authorises an upload — the permission only says
    // this person may work on the profile at all.
    //
    // Without this check the version test is the only guard, and it does not
    // guard the case that matters: while Alice has the profile open, Bob (who
    // also has Launch) can pull the current version, upload anything at all,
    // and Alice's whole session is refused at 409 when she closes her browser.
    // Her cookies are still on her disk, which is a recovery, not an outcome.
    // The token rather than the user id, so that a second machine signed in as
    // the same person cannot upload over the machine actually running it.
    let lock_token = header("x-fury-lock-token")
        .ok_or_else(|| ApiError::BadRequest("missing x-fury-lock-token".into()))?;
    let holds_lock: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM profile_locks \
         WHERE profile_id = $1 AND token_hash = $2 AND expires_at > now())",
    )
    .bind(profile_id)
    .bind(auth::hash_token(&lock_token))
    .fetch_one(&state.db)
    .await?;
    if !holds_lock {
        return Err(ApiError::Conflict(
            "this profile is not locked by you — it may have been taken over, or the lock \
             lapsed while the browser was running. Nothing was overwritten"
                .into(),
        ));
    }

    // Verified before anything is written. A truncated upload that is recorded
    // as a version is a profile that restores broken, and the operator finds
    // out on the machine that needed it.
    {
        use sha2::{Digest, Sha256};
        let actual: String = Sha256::digest(&body).iter().map(|b| format!("{b:02x}")).collect();
        if actual != sha256 {
            return Err(ApiError::BadRequest(
                "the body does not match the digest it was sent with".into(),
            ));
        }
    }

    let current: i32 = sqlx::query_scalar("SELECT current_version FROM profiles WHERE id = $1")
        .bind(profile_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    if base_version != current {
        return Err(ApiError::Conflict(format!(
            "this is based on version {base_version}, but the profile is at {current} —              someone else uploaded in between"
        )));
    }

    let version = current + 1;
    let dir = bundle_root().join(profile_id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::Internal(e.into()))?;
    let key = format!("{profile_id}/{version}.bundle");
    std::fs::write(bundle_root().join(&key), &body).map_err(|e| ApiError::Internal(e.into()))?;

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO bundles (id, profile_id, version, kind, s3_key, size_bytes, sha256,
                              wrapped_dek, uploaded_by)
         VALUES ($1, $2, $3, 'snapshot', $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(profile_id)
    .bind(version)
    .bind(&key)
    .bind(body.len() as i64)
    .bind(sha256.as_bytes())
    .bind(wrapped_key.as_bytes())
    .bind(caller.user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE profiles SET current_version = $2 WHERE id = $1")
        .bind(profile_id)
        .bind(version)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    audit(&state, &caller, "bundle.upload", Some(profile_id),
          json!({ "version": version, "bytes": body.len() })).await?;

    Ok(Json(json!({ "version": version })))
}

/// Fetch the current version.
async fn download_bundle(
    State(state): State<Arc<AppState>>,
    caller: Caller,
    Path(profile_id): Path<Uuid>,
) -> Result<axum::response::Response, ApiError> {
    let perms = rbac_guard::permissions_for_profile(&state.db, caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::Launch) {
        return Err(ApiError::Denied(Perm::Launch));
    }

    let row: Option<(i32, String, Vec<u8>)> = sqlx::query_as(
        "SELECT version, s3_key, wrapped_dek FROM bundles
         WHERE profile_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(profile_id)
    .fetch_optional(&state.db)
    .await?;

    let (version, key, wrapped) = row.ok_or(ApiError::NotFound)?;
    let path = bundle_root().join(&key);
    // The row and the bytes are in different places, so they can disagree: a
    // restore that brought back the database but not the disk, or a server
    // started with a different FURY_BUNDLE_DIR than the one that wrote them.
    // Both happen, and "No such file or directory" with no path in it is a
    // 500 that tells the operator nothing about which of the two it was.
    let bytes = std::fs::read(&path).map_err(|e| {
        ApiError::Internal(anyhow::anyhow!(
            "the database has version {version} of this profile but its bundle is not at {} ({e}). \
             Either FURY_BUNDLE_DIR points somewhere other than where it was written, or the \
             bundle directory was not restored with the database",
            path.display()
        ))
    })?;

    use axum::response::IntoResponse;
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert("x-fury-version", version.to_string().parse().unwrap());
    headers.insert(
        "x-fury-wrapped-key",
        String::from_utf8_lossy(&wrapped).parse().unwrap(),
    );
    Ok(response)
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
    Json(body): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    // Matched on the token, like the heartbeat, and not on the user alone.
    //
    // One person signed in on two machines is the ordinary case here — a laptop
    // and a desktop — and deleting by user id meant closing a profile on one of
    // them released the lock held by the other, where the browser was still
    // running and still writing cookies. The next colleague to ask would be
    // handed a profile that was in use.
    let token = body
        .get("lock_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("lock_token is required".into()))?;

    sqlx::query("DELETE FROM profile_locks WHERE profile_id = $1 AND token_hash = $2")
        .bind(profile_id)
        .bind(auth::hash_token(token))
        .execute(&state.db)
        .await?;

    // No error when nothing was deleted: the caller asked not to be holding
    // this profile, and after a force-release or a lapse they already are not.
    // Failing here would leave a client unable to tidy up after itself.
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
