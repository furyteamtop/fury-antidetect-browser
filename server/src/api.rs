// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

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
        .route("/v1/auth/signup", post(signup))
        .route("/v1/server", get(server_info))
        .route("/v1/me", get(whoami))
        .route("/v1/projects", get(list_projects).post(create_project))
        .route(
            "/v1/projects/{project_id}",
            axum::routing::patch(rename_project).delete(delete_project),
        )
        .route(
            "/v1/projects/{project_id}/grants",
            get(list_grants).post(grant_access),
        )
        .route(
            "/v1/projects/{project_id}/grants/{user_id}",
            axum::routing::delete(revoke_access),
        )
        .route("/v1/profiles", get(list_all_profiles))
        .route("/v1/profiles/move", post(move_profiles))
        .route("/v1/profiles/{profile_id}/clone", post(clone_profile))
        .route("/v1/profiles/trash", get(list_trash))
        .route("/v1/profiles/{profile_id}/restore", post(restore_profile))
        .route("/v1/profiles/{profile_id}/purge", axum::routing::delete(purge_profile))
        .route("/v1/proxies", get(list_proxies).post(create_proxy))
        .route(
            "/v1/proxies/{proxy_id}",
            axum::routing::patch(edit_proxy).delete(delete_proxy),
        )
        .route("/v1/proxies/{proxy_id}/credentials", get(reveal_proxy))
        .route(
            "/v1/profiles/{profile_id}",
            axum::routing::patch(edit_profile).delete(delete_profile),
        )
        .route(
            "/v1/projects/{project_id}/profiles",
            get(list_profiles).post(create_profile),
        )
        .route("/v1/org/members", get(list_members))
        .route("/v1/org/invitations", post(create_invitation))
        .route("/v1/org/members/{user_id}/key", post(hand_over_key))
        .route("/v1/org/rotation", get(rotation_material).post(rotate_org_key))
        .route(
            "/v1/profiles/{profile_id}/credentials",
            get(list_credentials).post(create_credential),
        )
        .route(
            "/v1/profiles/{profile_id}/credentials/{credential_id}",
            axum::routing::patch(edit_credential).delete(delete_credential),
        )
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
    let org: Option<(Uuid, Option<Vec<u8>>, String, Option<i32>)> = sqlx::query_as(
        "SELECT org_id, wrapped_ork, role::text, ork_generation \
         FROM org_members WHERE user_id = $1 LIMIT 1",
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
        "org_id": org.as_ref().map(|(id, _, _, _)| *id),
        "role": org.as_ref().map(|(_, _, r, _)| r.clone()),
        // None means enrolled but not yet handed the organisation key: they can
        // sign in and see the shape of the team, and decrypt nothing.
        "wrapped_ork": org.as_ref().and_then(|(_, ork, _, _)| ork.as_deref()).map(b64),
        // Which generation this wrapping belongs to.
        //
        // Without it a client cannot tell a current key from a retired one, and
        // the wrapping carries no signature — `seal_to` is anonymous by design.
        // A server that kept a member's pre-rotation blob could serve it back
        // and quietly return them to a key somebody else still holds. The
        // client refuses anything below the highest generation it has seen.
        "ork_generation": org.as_ref().and_then(|(_, _, _, g)| *g),
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
/// What a launcher can learn before anyone signs in.
///
/// One unauthenticated fact: whether this server lets a stranger make an
/// account. The launcher needs it to decide whether to offer the button, and
/// offering a button that always fails is its own kind of lie.
///
/// Deliberately nothing else. A server's name, its size, who is on it — all of
/// that would turn an address into a thing worth scanning for.
async fn server_info(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(json!({ "open_signup": state.open_signup }))
}

#[derive(Deserialize)]
pub struct SignupRequest {
    email: String,
    password: String,
    /// What to call the organisation this creates. Theirs alone.
    org_name: String,
    /// base64, as in enrolment. The server never parses these beyond decoding.
    public_key: String,
    wrapped_private_key: String,
    kdf_salt: String,
    /// Required: a sign-up always creates an organisation, so it always creates
    /// an owner, and an owner without the wrapped key owns nothing openable.
    wrapped_ork: String,
    #[serde(default)]
    machine_name: String,
}

/// Make an account with no invitation.
///
/// The only way onto a server used to be a code issued from its shell, which is
/// right for a team someone runs and wrong for a product someone downloads: a
/// person who installs Fury and wants their profiles on a server had nowhere to
/// go. This is that path.
///
/// Every sign-up creates its OWN organisation, with the caller as owner. That is
/// the whole safety story and it is structural rather than a check: an
/// organisation is the boundary every query already filters on, so a stranger
/// who signs up on your server cannot address anything of yours. They cost you
/// rows and nothing else.
async fn signup(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SignupRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if !state.open_signup {
        return Err(ApiError::BadRequest(
            "this server does not take open sign-ups. Ask whoever runs it for an \
             invitation, or set FURY_OPEN_SIGNUP=1 if it is yours"
                .into(),
        ));
    }

    // The ceiling on a shared server, checked before anything is written.
    //
    // Deleted organisations still count. An organisation is soft-deleted so its
    // profiles survive a mistake, and letting a delete-and-recreate loop past
    // the limit would make the limit decorative.
    if let Some(max) = state.limits.max_orgs {
        let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations")
            .fetch_one(&state.db)
            .await?;
        if existing >= max {
            return Err(ApiError::BadRequest(format!(
                "this server is full: it accepts {max} organisations and has {existing}. \
                 Ask whoever runs it for an invitation to an existing one"
            )));
        }
    }

    use base64::Engine;
    let b64 = |what: &str, v: &str| -> Result<Vec<u8>, ApiError> {
        base64::engine::general_purpose::STANDARD
            .decode(v)
            .map_err(|_| ApiError::BadRequest(format!("{what} is not base64")))
    };
    let public_key = b64("public_key", &req.public_key)?;
    let wrapped_private_key = b64("wrapped_private_key", &req.wrapped_private_key)?;
    let kdf_salt = b64("kdf_salt", &req.kdf_salt)?;
    let wrapped_ork = b64("wrapped_ork", &req.wrapped_ork)?;

    let email = req.email.trim().to_ascii_lowercase();
    if !email.contains('@') || email.len() < 3 {
        return Err(ApiError::BadRequest("that is not an email address".into()));
    }
    let org_name = req.org_name.trim();
    if org_name.is_empty() {
        return Err(ApiError::BadRequest("the organisation needs a name".into()));
    }
    // Same rule and the same sentence as enrolment: one requirement, said once.
    if req.password.len() < 12 {
        return Err(ApiError::BadRequest(
            "the password must be at least 12 characters: it is the only thing standing \
             between a stolen laptop and this organisation's accounts"
                .into(),
        ));
    }
    if public_key.len() != 32 {
        return Err(ApiError::BadRequest(
            "public_key must be 32 bytes — X25519".into(),
        ));
    }

    // One transaction: a user without an organisation cannot sign in and cannot
    // be repaired from the outside.
    let mut tx = state.db.begin().await?;

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
        sqlx::Error::Database(db) if db.is_unique_violation() => ApiError::BadRequest(
            "that address already has an account on this server. Sign in instead".into(),
        ),
        _ => ApiError::Db(e),
    })?;

    let org_id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(org_name)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO org_members (org_id, user_id, role, wrapped_ork, ork_generation) \
         VALUES ($1, $2, 'owner'::org_role, $3, 1)",
    )
    .bind(org_id)
    .bind(user_id)
    .bind(&wrapped_ork)
    .execute(&mut *tx)
    .await?;

    // Signed in on the way out, as enrolment does. Asking someone to retype the
    // password they chose ten seconds ago, into the same window, teaches them
    // nothing.
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

    tracing::info!(%user_id, %org_id, "signed up");
    Ok(Json(json!({
        "token": raw,
        "user_id": user_id,
        "org_id": org_id,
        "role": "owner",
        "has_org_key": true,
    })))
}

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
    mut db: auth::Db,
) -> ApiResult<Json<Vec<ProjectSummary>>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

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
    .fetch_all(db.as_mut())
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
    mut db: auth::Db,
    Json(req): Json<ProjectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

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
    .execute(db.as_mut())
    .await?;

    audit(db.as_mut(), &caller, "project.create", Some(id), json!({ "name": name })).await?;
    Ok(Json(json!({ "id": id })))
}

async fn rename_project(
    mut db: auth::Db,
    Path(project_id): Path<Uuid>,
    Json(req): Json<ProjectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    rbac_guard::require(db.as_mut(), caller.user_id, project_id, Perm::ManageAccess).await?;
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
    .execute(db.as_mut())
    .await?;

    audit(db.as_mut(), &caller, "project.rename", Some(project_id), json!({ "name": name })).await?;
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
    mut db: auth::Db,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    let mut tx = db.begin().await?;

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

    // rows_affected, not RETURNING. `RETURNING 1` in Postgres is an `integer`,
    // and this read it as `i64` — so sqlx refused to decode it and the whole
    // delete failed with a 500, but ONLY for a project that had profiles: an
    // empty one returns no rows, so nothing is decoded and nothing goes wrong.
    // "The empty project deletes and the full one does not" is exactly how it
    // was reported.
    let profiles = sqlx::query(
        "UPDATE profiles SET deleted_at = now() \
         WHERE project_id = $1 AND deleted_at IS NULL",
    )
    .bind(project_id)
    .execute(&mut *tx)
    .await?
    .rows_affected() as i64;

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
    audit(db.as_mut(), &caller, "project.delete", Some(project_id), json!({ "profiles": profiles })).await?;
    Ok(Json(json!({ "deleted": true, "profiles": profiles })))
}

async fn list_profiles(
    mut db: auth::Db,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ProfileSummary>>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    profiles_in(db.as_mut(), &caller.clone(), project_id).await
}

async fn profiles_in(
    db: &mut sqlx::PgConnection,
    caller: &Caller,
    project_id: Uuid,
) -> ApiResult<Json<Vec<ProfileSummary>>> {
    let perms = rbac_guard::permissions_for(&mut *db, caller.user_id, project_id).await?;
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
        String,
    )> = sqlx::query_as(
        &format!(
            r#"
        SELECT f.id, f.name, f.tags, f.persona_id, f.current_version,
               x.id, x.name, x.kind::text, x.host || ':' || x.port, x.last_country,
               l.user_id, u.email, l.machine_name,
               {acquired}, {expires}, p.name
        FROM profiles f
        JOIN projects p ON p.id = f.project_id
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
    .fetch_all(&mut *db)
    .await?;

    let out = rows
        .into_iter()
        .map(
            |(
                id, name, tags, persona_id, current_version,
                proxy_id, proxy_name, proxy_kind, proxy_host, proxy_country,
                lock_user, lock_email, lock_machine, lock_acquired, lock_expires,
                project_name,
            )| {
                ProfileSummary {
                    id,
                    project_id,
                    project_name,
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
                        // host:port in both cases. The first version showed
                        // only the host, and a provider selling fifty exits
                        // behind one gateway made every profile look identical.
                        display: match (&proxy_host, reveal) {
                            (Some(h), true) => h.clone(),
                            (Some(h), false) => match h.rsplit_once(':') {
                                Some((host, port)) => format!("{}:{port}", mask_host(host)),
                                None => mask_host(h),
                            },
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

/// Every profile the caller may see, across every project.
///
/// The interface's Profiles view is the flat list of everything an operator can
/// work with, and a project narrows it. Without this the team-mode version of
/// that screen was empty until somebody clicked a project — the first thing a
/// new member sees being "nothing here" while their work sits one click away.
///
/// Resolved per project rather than in one query with a join, because the
/// permissions differ per project and a row must carry the ones that apply to
/// it. It is a handful of small queries against an index, and the alternative
/// is a query whose correctness nobody can check by reading it.
async fn list_all_profiles(
    mut db: auth::Db,
) -> ApiResult<Json<Vec<ProfileSummary>>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    let projects: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT p.id FROM projects p \
         JOIN org_members m ON m.org_id = p.org_id AND m.user_id = $1 \
         LEFT JOIN project_grants g ON g.project_id = p.id AND g.user_id = $1 \
                AND (g.expires_at IS NULL OR g.expires_at > now()) \
         WHERE p.deleted_at IS NULL AND ($2 OR g.permissions IS NOT NULL) \
         ORDER BY p.name",
    )
    .bind(caller.user_id)
    .bind(fury_shared::rbac::has_implicit_project_access(caller.role))
    .fetch_all(db.as_mut())
    .await?;

    let mut out = Vec::new();
    for (project_id,) in projects {
        // Reusing the per-project handler rather than a second copy of its
        // query: two listings that drift apart is how one of them starts
        // showing an unmasked host.
        let Json(mut rows) = profiles_in(db.as_mut(), &caller, project_id).await?;
        out.append(&mut rows);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
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
    mut db: auth::Db,
    Path(project_id): Path<Uuid>,
    Json(req): Json<GrantRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    rbac_guard::require(db.as_mut(), caller.user_id, project_id, Perm::ManageAccess).await?;

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
    .fetch_optional(db.as_mut())
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
    .execute(db.as_mut())
    .await?;

    audit(db.as_mut(), &caller, "access.grant", Some(project_id),
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
    mut db: auth::Db,
    Path(project_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    rbac_guard::require(db.as_mut(), caller.user_id, project_id, Perm::ManageAccess).await?;

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
    .fetch_all(db.as_mut())
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
    .fetch_all(db.as_mut())
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
    mut db: auth::Db,
    Path((project_id, user_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    rbac_guard::require(db.as_mut(), caller.user_id, project_id, Perm::ManageAccess).await?;

    sqlx::query("DELETE FROM project_grants WHERE project_id = $1 AND user_id = $2")
        .bind(project_id)
        .bind(user_id)
        .execute(db.as_mut())
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
    .execute(db.as_mut())
    .await?;

    audit(db.as_mut(), &caller, "access.revoke", Some(project_id), json!({ "target": user_id })).await?;
    Ok(Json(json!({ "revoked": true })))
}

// ---------------------------------------------------------------------------
// proxies and profiles
// ---------------------------------------------------------------------------

fn unhex(what: &str, v: &str) -> Result<Vec<u8>, ApiError> {
    if v.len() % 2 != 0 || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::BadRequest(format!("{what} is not hex")));
    }
    (0..v.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&v[i..i + 2], 16).map_err(|_| ApiError::BadRequest(format!("{what} is not hex"))))
        .collect()
}

#[derive(Deserialize)]
pub struct ProxyRequest {
    name: String,
    kind: String,
    host: String,
    port: i32,
    /// NULL scopes the proxy to the whole organisation.
    #[serde(default)]
    project_id: Option<Uuid>,
    /// `{username, password}` sealed under a per-proxy data key, hex.
    credentials_enc: String,
    /// That data key, wrapped under the organisation key, hex.
    wrapped_dek: String,
    #[serde(default)]
    rotate_url_enc: Option<String>,
}

/// Add a proxy the team can use.
///
/// The server receives two opaque blobs and stores them. It cannot read either,
/// and does not try: no field here is parsed beyond checking it is hex and
/// plausible in length. That is the whole point — a stolen database yields a
/// list of hostnames, not a working proxy pool.
async fn create_proxy(
    mut db: auth::Db,
    Json(req): Json<ProxyRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::EditProxy));
    }
    if !matches!(req.kind.as_str(), "http" | "https" | "socks5") {
        return Err(ApiError::BadRequest("kind must be http, https or socks5".into()));
    }
    if !(1..=65535).contains(&req.port) {
        return Err(ApiError::BadRequest("port must be between 1 and 65535".into()));
    }
    if req.host.trim().is_empty() {
        return Err(ApiError::BadRequest("a proxy needs a host".into()));
    }

    let credentials_enc = unhex("credentials_enc", &req.credentials_enc)?;
    let wrapped_dek = unhex("wrapped_dek", &req.wrapped_dek)?;
    let rotate_url_enc = req
        .rotate_url_enc
        .as_deref()
        .map(|v| unhex("rotate_url_enc", v))
        .transpose()?;
    // 24-byte nonce plus a tag: anything shorter cannot be a sealed value, and
    // storing one would fail on an operator's machine at launch, with nothing
    // to point at.
    if wrapped_dek.len() < 40 || credentials_enc.len() < 40 {
        return Err(ApiError::BadRequest(
            "credentials_enc and wrapped_dek must be sealed values".into(),
        ));
    }

    // Scoped to the caller's organisation, and a project must belong to it.
    if let Some(project_id) = req.project_id {
        let ours: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id = $1 AND org_id = $2)",
        )
        .bind(project_id)
        .bind(caller.org_id)
        .fetch_one(db.as_mut())
        .await?;
        if !ours {
            return Err(ApiError::NotFound);
        }
    }

    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO proxies (id, org_id, project_id, name, kind, host, port,                               credentials_enc, wrapped_dek, rotate_url_enc)          VALUES ($1, $2, $3, $4, $5::proxy_kind, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(caller.org_id)
    .bind(req.project_id)
    .bind(req.name.trim())
    .bind(&req.kind)
    .bind(req.host.trim())
    .bind(req.port)
    .bind(&credentials_enc)
    .bind(&wrapped_dek)
    .bind(&rotate_url_enc)
    .execute(db.as_mut())
    .await?;

    audit(db.as_mut(), &caller, "proxy.create", Some(id), json!({ "host": req.host })).await?;
    Ok(Json(json!({ "id": id })))
}

/// The organisation's proxies, for the picker on a profile.
///
/// Never returns `credentials_enc`. Someone choosing which exit a profile uses
/// needs to tell them apart, which is a name and a masked host; the sealed
/// credentials are handed out once, bound to a lock, when a profile is actually
/// launched.
async fn list_proxies(
    mut db: auth::Db,
) -> ApiResult<Json<Vec<ProxySummary>>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    let reveal = caller.implicit_permissions().has(Perm::RevealSecrets);
    let rows: Vec<(Uuid, String, String, String, i32, Option<String>, Option<String>, Option<String>, i64)> =
        sqlx::query_as(&format!(
            "SELECT x.id, x.name, x.kind::text, x.host, x.port, x.last_country,                     x.last_kind_detected, {},                     (SELECT count(*) FROM profiles f WHERE f.proxy_id = x.id AND f.deleted_at IS NULL)              FROM proxies x              WHERE x.org_id = $1 AND x.deleted_at IS NULL              ORDER BY x.name",
            rfc3339("x.last_checked_at"),
        ))
        .bind(caller.org_id)
        .fetch_all(db.as_mut())
        .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, name, kind, host, port, country, detected, checked, used)| ProxySummary {
                id,
                name,
                kind,
                // The doc comment on ProxySummary promises host:port and the
                // first version gave only the host, so a picker could not tell
                // two ports on one gateway apart — which is how a provider
                // sells fifty exits.
                display: if reveal {
                    format!("{host}:{port}")
                } else {
                    format!("{}:{port}", mask_host(&host))
                },
                country,
                detected_kind: detected,
                last_checked_at: checked,
                shared_with_profiles: used,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
pub struct NewProfileRequest {
    name: String,
    #[serde(default)]
    tags: Vec<String>,
    persona_id: String,
    /// Sixteen lowercase hex characters. Generated by the client, because it is
    /// the client that has to keep it stable for the life of the account.
    fp_seed: String,
    timezone: String,
    languages: Vec<String>,
    proxy_id: Uuid,
    #[serde(default)]
    start_urls: Vec<String>,
    #[serde(default)]
    notes: String,
}

/// Create a profile in a project.
///
/// Everything a launch needs is required here rather than defaulted. A profile
/// that cannot be launched should not be creatable: the alternative is a row
/// that looks fine in a list and fails at the moment someone needs it, which
/// for a shared profile means failing on a colleague's machine.
async fn create_profile(
    State(state): State<Arc<AppState>>,
    mut db: auth::Db,
    Path(project_id): Path<Uuid>,
    Json(req): Json<NewProfileRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    rbac_guard::require(db.as_mut(), caller.user_id, project_id, Perm::CreateProfile).await?;
    ensure_room_for_a_profile(&mut db, &state.limits).await?;

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("a profile needs a name".into()));
    }
    let seed = fury_shared::persona::seed::from_hex(&req.fp_seed).ok_or_else(|| {
        ApiError::BadRequest("fp_seed must be sixteen hex characters".into())
    })?;
    // An empty timezone or language list is now a decision rather than an
    // omission: it means the profile follows its exit, and the agent resolves
    // both from the address the proxy actually leaves through. That is the
    // better default — a profile whose zone was pinned by hand months ago and
    // whose proxy has since moved country is exactly the contradiction the zone
    // was set to avoid.
    //
    // Stored as NULL and '{}' respectively, which is what launch_spec reads.
    if !fury_shared::catalogue::all().iter().any(|p| p.id == req.persona_id) {
        return Err(ApiError::BadRequest(format!(
            "no persona called {:?} — the client and the server ship different catalogues",
            req.persona_id
        )));
    }

    // The proxy has to be one of ours, and it has to be usable: a profile
    // pointing at a proxy whose credentials were never sealed is exactly the
    // unlaunchable row this endpoint refuses to create.
    let usable: Option<(bool,)> = sqlx::query_as(
        "SELECT (credentials_enc IS NOT NULL AND wrapped_dek IS NOT NULL)          FROM proxies WHERE id = $1 AND org_id = $2 AND deleted_at IS NULL",
    )
    .bind(req.proxy_id)
    .bind(caller.org_id)
    .fetch_optional(db.as_mut())
    .await?;
    match usable {
        None => return Err(ApiError::BadRequest("no such proxy".into())),
        Some((false,)) => {
            return Err(ApiError::BadRequest(
                "that proxy has no stored credentials, so nothing could launch through it".into(),
            ))
        }
        Some((true,)) => {}
    }

    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO profiles (id, org_id, project_id, name, notes, tags, persona_id, fp_seed,                                timezone, languages, proxy_id, start_urls, created_by)          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(caller.org_id)
    .bind(project_id)
    .bind(req.name.trim())
    .bind(&req.notes)
    .bind(&req.tags)
    .bind(&req.persona_id)
    .bind(fury_shared::persona::seed::to_bytes(seed).as_slice())
    .bind(Some(req.timezone.trim()).filter(|t| !t.is_empty()))
    .bind(&req.languages)
    .bind(req.proxy_id)
    .bind(&req.start_urls)
    .bind(caller.user_id)
    .execute(db.as_mut())
    .await?;

    audit(db.as_mut(), &caller, "profile.create", Some(id), json!({ "name": req.name })).await?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
pub struct CloneProfileRequest {
    count: u32,
    /// `{n}` is replaced by the copy's number. Absent means "<name> copy {n}".
    #[serde(default)]
    name: Option<String>,
}

/// Copy a profile's setup, never its identity.
///
/// Done here rather than in the client because the client does not have the
/// profile: the listing carries a name, tags, persona and proxy, and not the
/// timezone, languages, notes or start URLs. A copy assembled from the listing
/// would be silently missing four fields somebody had set on purpose.
///
/// What is NOT copied is the seed. It drives the canvas, audio and geometry
/// noise, so two profiles sharing one are byte-identical to any site that reads
/// a canvas and the accounts are linked for as long as they exist. A fresh one
/// per copy, generated here, is the whole point of the feature being a copy
/// rather than a duplicate. The bundle is not copied either — cookies ARE the
/// logged-in account, so a copy carrying them would be a second window on one
/// account instead of a second account.
/// A size somebody can act on.
///
/// Integer division made the first version of the storage refusal say "may
/// store 1 MB and has 0 MB", which reads as a bug rather than as a full disk —
/// the organisation had 700 KB. A refusal that looks wrong gets reported as a
/// fault instead of acted on.
fn human_bytes(n: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = 1024 * KB;
    const GB: i64 = 1024 * MB;
    match n {
        n if n >= GB => format!("{:.1} GB", n as f64 / GB as f64),
        n if n >= MB => format!("{:.1} MB", n as f64 / MB as f64),
        n if n >= KB => format!("{} KB", n / KB),
        n => format!("{n} bytes"),
    }
}

/// Room for one more profile in this organisation.
///
/// Called from every route that creates one — the dialog, the bulk maker, the
/// clone — because a ceiling one route enforces is a ceiling with a way round
/// it, and the way round is always the route somebody uses for two hundred at a
/// time.
///
/// Deleted profiles do not count: they are in the trash, they are recoverable,
/// and charging somebody for a profile they threw away would make the limit
/// feel arbitrary.
async fn ensure_room_for_a_profile(
    db: &mut auth::Db,
    limits: &crate::Limits,
) -> Result<(), ApiError> {
    ensure_room_for_profiles(db, limits, 1).await
}

async fn ensure_room_for_profiles(
    db: &mut auth::Db,
    limits: &crate::Limits,
    wanted: i64,
) -> Result<(), ApiError> {
    let Some(max) = limits.max_profiles_per_org else { return Ok(()) };
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM profiles WHERE org_id = $1 AND deleted_at IS NULL",
    )
    .bind(db.caller.org_id)
    .fetch_one(db.as_mut())
    .await?;
    if used + wanted > max {
        return Err(ApiError::BadRequest(format!(
            "this organisation may hold {max} profiles and holds {used}; \
             {wanted} more would not fit. Delete some, or ask whoever runs the \
             server for more room"
        )));
    }
    Ok(())
}

async fn clone_profile(
    State(state): State<Arc<AppState>>,
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
    Json(req): Json<CloneProfileRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    if !(1..=500).contains(&req.count) {
        return Err(ApiError::BadRequest("count must be between 1 and 500".into()));
    }
    // For all of them, not for one. A clone of five hundred is the route that
    // would otherwise walk straight past a ceiling checked one profile at a
    // time.
    ensure_room_for_profiles(&mut db, &state.limits, req.count as i64).await?;

    #[derive(sqlx::FromRow)]
    struct Row {
        project_id: Uuid,
        name: String,
        notes: String,
        tags: Vec<String>,
        persona_id: String,
        timezone: Option<String>,
        languages: Vec<String>,
        proxy_id: Option<Uuid>,
        start_urls: Vec<String>,
    }

    let source: Row = sqlx::query_as(
        "SELECT project_id, name, notes, tags, persona_id, timezone, languages, proxy_id, \
                start_urls \
         FROM profiles WHERE id = $1 AND org_id = $2 AND deleted_at IS NULL",
    )
    .bind(profile_id)
    .bind(caller.org_id)
    .fetch_optional(db.as_mut())
    .await?
    .ok_or(ApiError::NotFound)?;

    // Creating, not editing: the permission that matters is the one for adding
    // a profile to the project the copies land in.
    rbac_guard::require(db.as_mut(), caller.user_id, source.project_id, Perm::CreateProfile).await?;

    let pattern = match req.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if n.contains("{n}") => n.to_string(),
        Some(n) => format!("{n} {{n}}"),
        None => format!("{} copy {{n}}", source.name),
    };

    let mut created = Vec::new();
    for i in 1..=req.count {
        let id = Uuid::now_v7();
        let seed: u64 = {
            use rand::Rng;
            rand::thread_rng().gen()
        };
        sqlx::query(
            "INSERT INTO profiles (id, org_id, project_id, name, notes, tags, persona_id, \
                                   fp_seed, timezone, languages, proxy_id, start_urls, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(id)
        .bind(caller.org_id)
        .bind(source.project_id)
        .bind(pattern.replace("{n}", &i.to_string()))
        .bind(&source.notes)
        .bind(&source.tags)
        .bind(&source.persona_id)
        .bind(fury_shared::persona::seed::to_bytes(seed).as_slice())
        .bind(source.timezone.as_deref())
        .bind(&source.languages)
        .bind(source.proxy_id)
        .bind(&source.start_urls)
        .bind(caller.user_id)
        .execute(db.as_mut())
        .await?;
        created.push(id);
    }

    audit(
        db.as_mut(),
        &caller,
        "profile.clone",
        Some(profile_id),
        json!({ "count": req.count }),
    )
    .await?;
    Ok(Json(json!({ "created": created })))
}

#[derive(Deserialize)]
pub struct EditProxyRequest {
    name: String,
    kind: String,
    host: String,
    port: i32,
    /// Present only when the credentials changed. Absent leaves the stored ones
    /// alone, so renaming a proxy does not require the organisation key.
    #[serde(default)]
    credentials_enc: Option<String>,
    #[serde(default)]
    wrapped_dek: Option<String>,
}

/// Change a proxy.
///
/// The credentials are optional, and that is the useful part: renaming an exit
/// or correcting its port is an everyday edit, and requiring the organisation
/// key for it would mean a manager cannot fix a typo. Supplying them replaces
/// both halves together — a `credentials_enc` under one data key with a
/// `wrapped_dek` for another is a proxy that opens nowhere.
async fn edit_proxy(
    mut db: auth::Db,
    Path(proxy_id): Path<Uuid>,
    Json(req): Json<EditProxyRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::EditProxy));
    }
    if !matches!(req.kind.as_str(), "http" | "https" | "socks5") {
        return Err(ApiError::BadRequest("kind must be http, https or socks5".into()));
    }
    if !(1..=65535).contains(&req.port) || req.host.trim().is_empty() {
        return Err(ApiError::BadRequest("a proxy needs a host and a port".into()));
    }

    let credentials = match (&req.credentials_enc, &req.wrapped_dek) {
        (Some(c), Some(d)) => {
            let c = unhex("credentials_enc", c)?;
            let d = unhex("wrapped_dek", d)?;
            if c.len() < 40 || d.len() < 40 {
                return Err(ApiError::BadRequest(
                    "credentials_enc and wrapped_dek must be sealed values".into(),
                ));
            }
            Some((c, d))
        }
        (None, None) => None,
        _ => {
            return Err(ApiError::BadRequest(
                "credentials_enc and wrapped_dek travel together or not at all".into(),
            ))
        }
    };

    let updated = match credentials {
        Some((c, d)) => {
            sqlx::query(
                "UPDATE proxies SET name = $1, kind = $2::proxy_kind, host = $3, port = $4, \
                        credentials_enc = $5, wrapped_dek = $6 \
                 WHERE id = $7 AND org_id = $8 AND deleted_at IS NULL",
            )
            .bind(req.name.trim())
            .bind(&req.kind)
            .bind(req.host.trim())
            .bind(req.port)
            .bind(&c)
            .bind(&d)
            .bind(proxy_id)
            .bind(caller.org_id)
            .execute(db.as_mut())
            .await?
        }
        None => {
            sqlx::query(
                "UPDATE proxies SET name = $1, kind = $2::proxy_kind, host = $3, port = $4 \
                 WHERE id = $5 AND org_id = $6 AND deleted_at IS NULL",
            )
            .bind(req.name.trim())
            .bind(&req.kind)
            .bind(req.host.trim())
            .bind(req.port)
            .bind(proxy_id)
            .bind(caller.org_id)
            .execute(db.as_mut())
            .await?
        }
    };
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    audit(db.as_mut(), &caller, "proxy.edit", Some(proxy_id),
          json!({ "credentials_changed": req.credentials_enc.is_some() })).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Hand over one proxy's sealed credentials.
///
/// Everywhere else, ciphertext is released only as part of taking a lock, so
/// that every handout is already recorded as a launch. Checking a proxy is the
/// exception that has to exist — an operator must be able to ask "does this
/// exit still work" without opening a profile — so it is its own permission and
/// its own audit line rather than a quiet reuse of something else.
///
/// Still ciphertext. The server has never held the plaintext and does not here.
async fn reveal_proxy(
    mut db: auth::Db,
    Path(proxy_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    if !caller.implicit_permissions().has(Perm::RevealSecrets) {
        return Err(ApiError::Denied(Perm::RevealSecrets));
    }

    let row: Option<(String, String, i32, Option<Vec<u8>>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT kind::text, host, port, credentials_enc, wrapped_dek \
         FROM proxies WHERE id = $1 AND org_id = $2 AND deleted_at IS NULL",
    )
    .bind(proxy_id)
    .bind(caller.org_id)
    .fetch_optional(db.as_mut())
    .await?;

    let (kind, host, port, credentials_enc, wrapped_dek) = row.ok_or(ApiError::NotFound)?;
    let (Some(credentials_enc), Some(wrapped_dek)) = (credentials_enc, wrapped_dek) else {
        return Err(ApiError::BadRequest(
            "this proxy has no stored credentials".into(),
        ));
    };

    audit(db.as_mut(), &caller, "proxy.reveal", Some(proxy_id), json!({})).await?;

    let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Ok(Json(json!({
        "kind": kind,
        "host": host,
        "port": port,
        "credentials_enc": hex(&credentials_enc),
        "wrapped_dek": hex(&wrapped_dek),
    })))
}

/// Retire a proxy.
///
/// Soft, and the profiles that used it keep working until someone gives them
/// another — `ON DELETE SET NULL`, because a warmed account is worth more than
/// the exit it happened to go out through. They will refuse to launch, with a
/// sentence saying why, which is the correct amount of stopping.
async fn delete_proxy(
    mut db: auth::Db,
    Path(proxy_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::EditProxy));
    }
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM profiles WHERE proxy_id = $1 AND deleted_at IS NULL",
    )
    .bind(proxy_id)
    .fetch_one(db.as_mut())
    .await?;

    let updated = sqlx::query(
        "UPDATE proxies SET deleted_at = now() \
         WHERE id = $1 AND org_id = $2 AND deleted_at IS NULL",
    )
    .bind(proxy_id)
    .bind(caller.org_id)
    .execute(db.as_mut())
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    audit(db.as_mut(), &caller, "proxy.delete", Some(proxy_id), json!({ "profiles": used })).await?;
    Ok(Json(json!({ "deleted": true, "profiles_left_without_a_proxy": used })))
}

#[derive(Deserialize)]
pub struct EditProfileRequest {
    name: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    tags: Vec<String>,
    timezone: String,
    languages: Vec<String>,
    proxy_id: Uuid,
    #[serde(default)]
    start_urls: Vec<String>,
}

/// Change a profile. Never its persona or its seed.
///
/// Those two are the machine the account has been seen on, and changing them is
/// not an edit — it is telling a site that a device it has known for months has
/// been replaced. Nothing in this API can do it, which is why there is no field
/// for it here rather than a check that rejects one.
async fn edit_profile(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
    Json(req): Json<EditProfileRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::EditProfile) {
        return Err(ApiError::Denied(Perm::EditProfile));
    }

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("a profile needs a name".into()));
    }
    // Empty means "follow the exit" here too — clearing the field is how an
    // operator hands the decision back to the proxy. See create_profile.

    // Changing which exit a profile uses is its own permission: it is the one
    // edit that changes where the account appears to be.
    let current: Option<Uuid> = sqlx::query_scalar("SELECT proxy_id FROM profiles WHERE id = $1")
        .bind(profile_id)
        .fetch_one(db.as_mut())
        .await?;
    if current != Some(req.proxy_id) && !perms.has(Perm::EditProxy) {
        return Err(ApiError::Denied(Perm::EditProxy));
    }

    let usable: Option<(bool,)> = sqlx::query_as(
        "SELECT (credentials_enc IS NOT NULL AND wrapped_dek IS NOT NULL) \
         FROM proxies WHERE id = $1 AND org_id = $2 AND deleted_at IS NULL",
    )
    .bind(req.proxy_id)
    .bind(caller.org_id)
    .fetch_optional(db.as_mut())
    .await?;
    match usable {
        None => return Err(ApiError::BadRequest("no such proxy".into())),
        Some((false,)) => {
            return Err(ApiError::BadRequest(
                "that proxy has no stored credentials, so nothing could launch through it".into(),
            ))
        }
        Some((true,)) => {}
    }

    sqlx::query(
        "UPDATE profiles SET name = $1, notes = $2, tags = $3, timezone = $4, \
                             languages = $5, proxy_id = $6, start_urls = $7 \
         WHERE id = $8 AND deleted_at IS NULL",
    )
    .bind(req.name.trim())
    .bind(&req.notes)
    .bind(&req.tags)
    .bind(Some(req.timezone.trim()).filter(|t| !t.is_empty()))
    .bind(&req.languages)
    .bind(req.proxy_id)
    .bind(&req.start_urls)
    .bind(profile_id)
    .execute(db.as_mut())
    .await?;

    audit(db.as_mut(), &caller, "profile.edit", Some(profile_id), json!({ "name": req.name })).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Send a profile to the trash.
///
/// Soft, always. A profile holds a warmed account, and the difference between
/// hiding one and destroying one must never be a single click — the same rule
/// the local store follows. Refused while it is open: a browser closing after
/// this would push a bundle into a profile nobody can find.
async fn delete_profile(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::DeleteProfile) {
        return Err(ApiError::Denied(Perm::DeleteProfile));
    }

    let open: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM profile_locks WHERE profile_id = $1 AND expires_at > now())",
    )
    .bind(profile_id)
    .fetch_one(db.as_mut())
    .await?;
    if open {
        return Err(ApiError::Conflict(
            "this profile is open right now. Close it first — a browser closing after it was \
             deleted would save into a profile nobody can find"
                .into(),
        ));
    }

    let updated = sqlx::query("UPDATE profiles SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL")
        .bind(profile_id)
        .execute(db.as_mut())
        .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    audit(db.as_mut(), &caller, "profile.delete", Some(profile_id), json!({})).await?;
    Ok(Json(json!({ "deleted": true })))
}

#[derive(Deserialize)]
pub struct MoveRequest {
    ids: Vec<Uuid>,
    /// Where they land. Required: on a server a profile always belongs to a
    /// project, because the project is what carries access to it — a profile
    /// outside every project would be one nobody has any stated right to open.
    project_id: Uuid,
}

/// Move profiles into another project.
///
/// Two permissions, not one, and they are on different projects. Taking a
/// profile out of a project removes it from everyone who could reach it there,
/// which is `delete_profile` on the source; putting it into another makes it
/// reachable by everyone who can reach that one, which is `create_profile` on
/// the destination. Checking only the destination would let anyone who can
/// create a profile anywhere quietly move somebody else's work into a project
/// only they can see.
async fn move_profiles(
    mut db: auth::Db,
    Json(req): Json<MoveRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    if req.ids.is_empty() {
        return Ok(Json(json!({ "moved": 0 })));
    }
    rbac_guard::require(db.as_mut(), caller.user_id, req.project_id, Perm::CreateProfile).await?;

    let mut tx = db.begin().await?;
    let mut moved = 0;

    for id in &req.ids {
        let row: Option<(Uuid, bool)> = sqlx::query_as(
            "SELECT f.project_id, \
                    EXISTS(SELECT 1 FROM profile_locks l \
                            WHERE l.profile_id = f.id AND l.expires_at > now()) \
             FROM profiles f \
             WHERE f.id = $1 AND f.org_id = $2 AND f.deleted_at IS NULL",
        )
        .bind(id)
        .bind(caller.org_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some((from, open)) = row else {
            return Err(ApiError::NotFound);
        };
        if from == req.project_id {
            continue;
        }
        if open {
            // Mid-session, the set of people who can reach this profile would
            // change under the person using it — and the bundle they push on
            // close would land in a project they were never granted.
            return Err(ApiError::Conflict(
                "one of these profiles is open right now. Close it first".into(),
            ));
        }

        // Resolved against the source project each time rather than once: a
        // selection can span projects, and a caller with rights on one of them
        // must not carry those rights to the rest.
        let perms = rbac_guard::permissions_for(&mut *tx, caller.user_id, from).await?;
        if !perms.has(Perm::DeleteProfile) {
            return Err(ApiError::Denied(Perm::DeleteProfile));
        }

        sqlx::query("UPDATE profiles SET project_id = $1 WHERE id = $2")
            .bind(req.project_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        moved += 1;
    }

    tx.commit().await?;
    audit(db.as_mut(), &caller, "profile.move", Some(req.project_id),
          json!({ "profiles": req.ids, "moved": moved })).await?;
    Ok(Json(json!({ "moved": moved })))
}

/// Deleted profiles the caller may see.
///
/// Deletion is soft everywhere in this product for one reason: a profile holds
/// an account that took months to warm, and the difference between hiding one
/// and destroying one must never be a single click.
async fn list_trash(
    mut db: auth::Db,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    let rows: Vec<(Uuid, Uuid, String, String, Vec<String>, String, String)> =
        sqlx::query_as(&format!(
            "SELECT f.id, f.project_id, p.name, f.name, f.tags, f.persona_id, {} \
             FROM profiles f \
             JOIN projects p ON p.id = f.project_id \
             JOIN org_members m ON m.org_id = p.org_id AND m.user_id = $1 \
             LEFT JOIN project_grants g ON g.project_id = p.id AND g.user_id = $1 \
                    AND (g.expires_at IS NULL OR g.expires_at > now()) \
             WHERE f.deleted_at IS NOT NULL AND ($2 OR g.permissions IS NOT NULL) \
             ORDER BY f.deleted_at DESC",
            rfc3339("f.deleted_at"),
        ))
        .bind(caller.user_id)
        .bind(fury_shared::rbac::has_implicit_project_access(caller.role))
        .fetch_all(db.as_mut())
        .await?;

    Ok(Json(json!(rows
        .into_iter()
        .map(|(id, project_id, project_name, name, tags, persona_id, deleted_at)| json!({
            "id": id,
            "project_id": project_id,
            "project_name": project_name,
            "name": name,
            "tags": tags,
            "persona_id": persona_id,
            "fp_seed": 0,
            "proxy": serde_json::Value::Null,
            // The trash answers "when did this go", not "when did it last run".
            "last_opened_at": deleted_at,
            "running": false,
        }))
        .collect::<Vec<_>>())))
}

async fn restore_profile(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    // Resolved against the deleted row, which permissions_for_profile cannot
    // see — it filters deleted profiles, as every other caller needs it to.
    let project: Option<(Uuid,)> = sqlx::query_as("SELECT project_id FROM profiles WHERE id = $1")
        .bind(profile_id)
        .fetch_optional(db.as_mut())
        .await?;
    let (project_id,) = project.ok_or(ApiError::NotFound)?;
    rbac_guard::require(db.as_mut(), caller.user_id, project_id, Perm::DeleteProfile).await?;

    // Into a project that still exists, or nowhere useful. A profile restored
    // into a deleted project is invisible again, which is the state the trash
    // exists to get it out of.
    let alive: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(project_id)
    .fetch_one(db.as_mut())
    .await?;
    if !alive {
        return Err(ApiError::BadRequest(
            "the project this profile was in has been deleted. Restore the project first"
                .into(),
        ));
    }

    sqlx::query("UPDATE profiles SET deleted_at = NULL WHERE id = $1")
        .bind(profile_id)
        .execute(db.as_mut())
        .await?;
    audit(db.as_mut(), &caller, "profile.restore", Some(profile_id), json!({})).await?;
    Ok(Json(json!({ "restored": true })))
}

/// Gone for good: the row, and every bundle stored for it.
async fn purge_profile(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    let project: Option<(Uuid,)> = sqlx::query_as("SELECT project_id FROM profiles WHERE id = $1")
        .bind(profile_id)
        .fetch_optional(db.as_mut())
        .await?;
    let (project_id,) = project.ok_or(ApiError::NotFound)?;
    rbac_guard::require(db.as_mut(), caller.user_id, project_id, Perm::DeleteProfile).await?;

    // The bytes go first. A row deleted while its bundles stay is a directory
    // of live cookie jars for a profile the operator believes is destroyed —
    // the opposite of what emptying a trash means.
    let dir = bundle_root().join(profile_id.to_string());
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| {
            ApiError::Internal(anyhow::anyhow!("could not remove {}: {e}", dir.display()))
        })?;
    }
    sqlx::query("DELETE FROM profiles WHERE id = $1 AND deleted_at IS NOT NULL")
        .bind(profile_id)
        .execute(db.as_mut())
        .await?;

    audit(db.as_mut(), &caller, "profile.purge", Some(profile_id), json!({})).await?;
    Ok(Json(json!({ "purged": true })))
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
    mut db: auth::Db,
    Json(req): Json<InviteRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;

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
    .fetch_one(db.as_mut())
    .await?;
    if exists {
        return Err(ApiError::BadRequest(format!("{email} is already in this team")));
    }

    let code = crate::enroll::issue(
        db.as_mut(),
        email,
        crate::enroll::Org::Existing(caller.org_id),
        &req.role,
    )
    .await
    .map_err(ApiError::Internal)?;

    audit(db.as_mut(), &caller, "member.invite", None, json!({ "email": email, "role": req.role })).await?;

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
    mut db: auth::Db,
    Path(user_id): Path<Uuid>,
    Json(req): Json<HandOverRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;

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
        .fetch_one(db.as_mut())
        .await?;

    let updated = sqlx::query(
        "UPDATE org_members SET wrapped_ork = $1, ork_generation = $2 \
         WHERE org_id = $3 AND user_id = $4",
    )
    .bind(&wrapped)
    .bind(generation)
    .bind(caller.org_id)
    .bind(user_id)
    .execute(db.as_mut())
    .await?;

    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    audit(db.as_mut(), &caller, "member.key", None, json!({ "target": user_id })).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Everything a client needs to compute a new organisation key.
///
/// Only the material, never a decision: the public keys the new key must be
/// sealed to, and the wrapped data keys that must be re-wrapped under it. The
/// server cannot open any of it and does not try.
async fn rotation_material(
    mut db: auth::Db,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    use base64::Engine;
    let b64 = |v: &[u8]| base64::engine::general_purpose::STANDARD.encode(v);
    let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();

    // Only members who already hold the key.
    //
    // Enrolling makes an account; being handed the organisation key is a
    // separate, deliberate act, and `has_key: false` is the gate an admin sees.
    // Without this predicate a rotation walked straight through it: everyone
    // who had ever enrolled — including someone an admin looked at and chose
    // not to admit — was sealed the new key by an operation nobody associates
    // with granting access. Worse, the completeness check below made omitting
    // them impossible.
    let members: Vec<(Uuid, String, Vec<u8>)> = sqlx::query_as(
        "SELECT u.id, u.email, u.public_key FROM org_members m \
         JOIN users u ON u.id = m.user_id \
         WHERE m.org_id = $1 AND u.disabled_at IS NULL AND m.wrapped_ork IS NOT NULL",
    )
    .bind(caller.org_id)
    .fetch_all(db.as_mut())
    .await?;

    let proxies: Vec<(Uuid, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT id, wrapped_dek FROM proxies WHERE org_id = $1 AND deleted_at IS NULL",
    )
    .bind(caller.org_id)
    .fetch_all(db.as_mut())
    .await?;

    let generation: i32 = sqlx::query_scalar("SELECT ork_generation FROM organizations WHERE id = $1")
        .bind(caller.org_id)
        .fetch_one(db.as_mut())
        .await?;

    Ok(Json(json!({
        "generation": generation,
        "members": members.iter().map(|(id, email, pk)| json!({
            "user_id": id, "email": email, "public_key": b64(pk),
        })).collect::<Vec<_>>(),
        // A proxy with no stored key has nothing to re-wrap; it is listed so
        // the client can report it rather than silently leaving it behind.
        "proxies": proxies.iter().map(|(id, dek)| json!({
            "id": id, "wrapped_dek": dek.as_deref().map(hex),
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct RotateRequest {
    /// Whose membership this rotation is retiring. `None` rotates without
    /// removing anyone — after a laptop is lost, say.
    #[serde(default)]
    remove_user_id: Option<Uuid>,
    /// The new key, sealed to every member who stays. Must be all of them.
    members: Vec<RotatedMember>,
    /// Every proxy data key, re-wrapped under the new organisation key.
    proxies: Vec<RotatedProxy>,
    /// The generation this was computed against, so two administrators
    /// rotating at once cannot half-apply each other's work.
    generation: i32,
}

#[derive(Deserialize)]
pub struct RotatedMember {
    user_id: Uuid,
    wrapped_ork: String,
}

#[derive(Deserialize)]
pub struct RotatedProxy {
    id: Uuid,
    wrapped_dek: String,
}

/// Replace the organisation key.
///
/// This is what makes removing somebody mean anything. A member who leaves
/// keeps whatever they already downloaded — that is unavoidable, the work
/// happened on their machine — but without rotation they also keep a key that
/// opens everything the team does *afterwards*, which is not a leak, it is
/// continued access. Deleting a grant while leaving the key in their hands is
/// the security equivalent of changing the sign on the door.
///
/// Everything lands in one transaction. A half-rotated organisation is one
/// where some proxies open under the old key and some under the new, and no
/// single member can read all of them.
async fn rotate_org_key(
    mut db: auth::Db,
    Json(req): Json<RotateRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    use fury_shared::rbac::OrgRole;
    if !matches!(caller.role, OrgRole::Owner | OrgRole::Admin) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }
    if req.remove_user_id == Some(caller.user_id) {
        return Err(ApiError::BadRequest(
            "you cannot remove yourself — the organisation would have nobody who can rotate the \
             key again"
                .into(),
        ));
    }

    let mut tx = db.begin().await?;

    // Locked for the duration, so a second administrator rotating at the same
    // moment waits rather than interleaving.
    let generation: i32 = sqlx::query_scalar(
        "SELECT ork_generation FROM organizations WHERE id = $1 FOR UPDATE",
    )
    .bind(caller.org_id)
    .fetch_one(&mut *tx)
    .await?;
    if generation != req.generation {
        return Err(ApiError::Conflict(format!(
            "this was computed against generation {} but the organisation is at {generation} — \
             somebody else rotated in between. Start again",
            req.generation
        )));
    }

    if let Some(gone) = req.remove_user_id {
        let removed = sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND user_id = $2")
            .bind(caller.org_id)
            .bind(gone)
            .execute(&mut *tx)
            .await?;
        if removed.rows_affected() == 0 {
            return Err(ApiError::NotFound);
        }
        // Their grants and their sessions go with them. Leaving a live session
        // would let a removed member keep working until it expired.
        sqlx::query("DELETE FROM project_grants WHERE user_id = $1")
            .bind(gone)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(gone)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM profile_locks WHERE user_id = $1")
            .bind(gone)
            .execute(&mut *tx)
            .await?;
    }

    // Every remaining member must be covered. One left out is one who can no
    // longer decrypt anything, discovered later and unfixable without another
    // rotation.
    // Same predicate as the material, or the two disagree and one of them
    // wins silently. A member with no key cannot be "locked out" by a rotation
    // — they were never let in.
    let remaining: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM org_members WHERE org_id = $1 AND wrapped_ork IS NOT NULL",
    )
    .bind(caller.org_id)
    .fetch_all(&mut *tx)
    .await?;
    let supplied: std::collections::HashSet<Uuid> =
        req.members.iter().map(|m| m.user_id).collect();
    for (user_id,) in &remaining {
        if !supplied.contains(user_id) {
            return Err(ApiError::BadRequest(format!(
                "no new key was supplied for member {user_id}; they would be locked out"
            )));
        }
    }

    let next = generation + 1;
    for m in &req.members {
        let wrapped = unhex_or_b64("wrapped_ork", &m.wrapped_ork)?;
        if wrapped.len() < 104 {
            return Err(ApiError::BadRequest(
                "that is not a sealed organisation key".into(),
            ));
        }
        // `wrapped_ork IS NOT NULL` here too, so a client that supplies an
        // entry for a member who was never admitted writes nothing. Three
        // places, one rule: rotation re-keys those who already hold a key, and
        // admitting someone is `hand_over_key` and nothing else.
        sqlx::query(
            "UPDATE org_members SET wrapped_ork = $1, ork_generation = $2 \
             WHERE org_id = $3 AND user_id = $4 AND wrapped_ork IS NOT NULL",
        )
        .bind(&wrapped)
        .bind(next)
        .bind(caller.org_id)
        .bind(m.user_id)
        .execute(&mut *tx)
        .await?;
    }

    for x in &req.proxies {
        let wrapped = unhex("wrapped_dek", &x.wrapped_dek)?;
        if wrapped.len() < 40 {
            return Err(ApiError::BadRequest("that is not a wrapped data key".into()));
        }
        sqlx::query("UPDATE proxies SET wrapped_dek = $1 WHERE id = $2 AND org_id = $3")
            .bind(&wrapped)
            .bind(x.id)
            .bind(caller.org_id)
            .execute(&mut *tx)
            .await?;
    }

    sqlx::query("UPDATE organizations SET ork_generation = $1 WHERE id = $2")
        .bind(next)
        .bind(caller.org_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    audit(db.as_mut(), &caller, "org.rotate", None,
          json!({ "removed": req.remove_user_id, "generation": next })).await?;
    Ok(Json(json!({ "generation": next, "removed": req.remove_user_id })))
}

/// Accepts hex, or base64 — `hand_over_key` established base64 for a sealed
/// organisation key and the proxy endpoints hex for everything else. Rather
/// than pick one and break the other, this takes either.
fn unhex_or_b64(what: &str, v: &str) -> Result<Vec<u8>, ApiError> {
    if v.len() % 2 == 0 && v.bytes().all(|b| b.is_ascii_hexdigit()) {
        return unhex(what, v);
    }
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(v)
        .map_err(|_| ApiError::BadRequest(format!("{what} is neither hex nor base64")))
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
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
    Json(req): Json<LockRequest>,
) -> ApiResult<Json<AcquireLockResponse>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::Launch) {
        return Err(ApiError::Denied(Perm::Launch));
    }
    if req.force && !perms.has(Perm::ManageAccess) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    // Assembled before the lock is taken. A lock held on a profile that turns
    // out to be unlaunchable is a profile nobody else can open for the next
    // ninety seconds, over a failure that was knowable up front.
    let spec = launch_spec(db.as_mut(), profile_id).await?;

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
    .fetch_optional(db.as_mut())
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
        .fetch_optional(db.as_mut())
        .await?;
        let (email, machine) = holder.unwrap_or_default();
        return Err(ApiError::Locked(
            json!({ "user_email": email, "machine_name": machine }),
        ));
    }

    if req.force {
        audit(db.as_mut(), &caller, "lock.force", Some(profile_id), json!({})).await?;
    }
    audit(db.as_mut(), &caller, "profile.launch", Some(profile_id),
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
    .fetch_one(db.as_mut())
    .await?;
    let expires_at = expires_at.0;

    Ok(Json(AcquireLockResponse {
        lock_token: raw,
        expires_at: expires_at.to_string(),
        restrictions,
        spec,
    }))
}

/// Everything needed to start one profile.
///
/// Refuses, with a sentence rather than a code, when the profile is missing
/// something a launch cannot proceed without. The wording mirrors the agent's
/// own (`agent/src/ipc.rs`), because an operator should read the same
/// explanation whether the profile lives on this machine or on a server.
async fn launch_spec(db: &mut sqlx::PgConnection, profile_id: Uuid) -> ApiResult<fury_shared::api::LaunchSpec> {
    #[derive(sqlx::FromRow)]
    struct Row {
        name: String,
        persona_id: String,
        fp_seed: Vec<u8>,
        timezone: Option<String>,
        languages: Vec<String>,
        start_urls: Vec<String>,
        px_id: Option<Uuid>,
        px_kind: Option<String>,
        px_host: Option<String>,
        px_port: Option<i32>,
        credentials_enc: Option<Vec<u8>>,
        wrapped_dek: Option<Vec<u8>>,
    }

    let row: Row = sqlx::query_as(
        "SELECT f.name, f.persona_id, f.fp_seed, f.timezone, f.languages, f.start_urls,                 x.id AS px_id, x.kind::text AS px_kind, x.host AS px_host, x.port AS px_port,                 x.credentials_enc, x.wrapped_dek          FROM profiles f          LEFT JOIN proxies x ON x.id = f.proxy_id AND x.deleted_at IS NULL          WHERE f.id = $1 AND f.deleted_at IS NULL",
    )
    .bind(profile_id)
    .fetch_optional(&mut *db)
    .await?
    .ok_or(ApiError::NotFound)?;

    let (Some(px_id), Some(kind), Some(host), Some(port)) =
        (row.px_id, row.px_kind, row.px_host, row.px_port)
    else {
        return Err(ApiError::BadRequest(
            "this profile has no proxy. Everything the browser does goes through one, so              launching without it would send traffic from the operator's own address"
                .into(),
        ));
    };
    let (Some(credentials_enc), Some(wrapped_dek)) = (row.credentials_enc, row.wrapped_dek) else {
        return Err(ApiError::BadRequest(format!(
            "the proxy on this profile has no stored credentials. It was created before              credentials were sealed, or by a client that could not seal them — open it in              Proxies and save it again ({px_id})"
        )));
    };
    // Absence is the instruction: no stored timezone means the profile follows
    // its exit, and the agent resolves the zone from the address the proxy
    // actually leaves through — which it can do and this server cannot, having
    // never seen the exit and holding no proxy credentials.
    //
    // One rule, and the same one a local profile uses. This used to refuse a
    // NULL outright, so the shell always sent a string, so the agent's
    // follow-the-exit branch was dead code for every team profile: the feature
    // existed and only local profiles could reach it.
    //
    // Deliberately NOT keyed on the `auto_timezone` / `auto_locale` columns
    // that have sat in the schema since the first migration. Nothing had ever
    // read them, every existing row has them TRUE by default, and honouring
    // them now would discard the timezone every one of those profiles has
    // stored. Two representations of one fact, one of which is stale — the
    // migration beside this change drops them.
    let timezone = row.timezone;
    let languages = (!row.languages.is_empty()).then_some(row.languages);

    let fp_seed = fury_shared::persona::seed::from_bytes(&row.fp_seed)
        .map(fury_shared::persona::seed::to_hex)
        .ok_or_else(|| {
            ApiError::Internal(anyhow::anyhow!(
                "profile {profile_id} has a {}-byte seed; it must be 8",
                row.fp_seed.len()
            ))
        })?;

    Ok(fury_shared::api::LaunchSpec {
        profile_id,
        name: row.name,
        persona_id: row.persona_id,
        fp_seed,
        timezone,
        languages,
        start_urls: row.start_urls,
        proxy: fury_shared::api::SealedProxy {
            id: px_id,
            kind,
            host,
            port: port as u16,
            credentials_enc,
            wrapped_dek,
        },
    })
}

// ---------------------------------------------------------------------------
// credentials
// ---------------------------------------------------------------------------
//
// A login that belongs to a profile: the username, the password, and the
// two-factor seed. The server holds a blob it cannot read and a wrapped key it
// cannot open, exactly as it does for proxy credentials and profile bundles.
//
// Gated on RevealSecrets rather than on Launch, and the distinction is the
// point of the permission. Anyone who may open a profile is working inside a
// logged-in browser and never needs to see the password; the person who needs
// the password is the one setting the account up, or recovering it, and that is
// a different and smaller group. An operator who can launch but not reveal can
// do their job and cannot take the account home.

#[derive(Deserialize)]
pub struct CredentialBody {
    label: String,
    #[serde(default)]
    site: String,
    /// Hex. `{username, password, totp, notes}` sealed under a data key.
    payload_enc: String,
    /// Hex. That data key, wrapped under the organisation key.
    wrapped_dek: String,
}

async fn list_credentials(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::RevealSecrets) {
        return Err(ApiError::Denied(Perm::RevealSecrets));
    }

    let rows: Vec<(Uuid, String, String, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT id, label, site, payload_enc, wrapped_dek FROM credentials
         WHERE profile_id = $1 ORDER BY label",
    )
    .bind(profile_id)
    .fetch_all(db.as_mut())
    .await?;

    // Reading a login is worth recording. A password taken out of a shared
    // account is the event an investigation starts from, and "who saw it and
    // when" is the only question that gets asked.
    if !rows.is_empty() {
        audit(db.as_mut(), &caller, "credential.reveal", Some(profile_id),
              json!({ "count": rows.len() })).await?;
    }

    Ok(Json(json!(rows
        .into_iter()
        .map(|(id, label, site, payload, dek)| json!({
            "id": id,
            "label": label,
            "site": site,
            "payload_enc": String::from_utf8_lossy(&payload),
            "wrapped_dek": String::from_utf8_lossy(&dek),
        }))
        .collect::<Vec<_>>())))
}

async fn create_credential(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
    Json(req): Json<CredentialBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::RevealSecrets) {
        return Err(ApiError::Denied(Perm::RevealSecrets));
    }
    // The same shape check the proxy credentials get: a client that forgot to
    // seal sends something short, and storing it would put a password in the
    // database in the clear while every part of this file claimed otherwise.
    if req.payload_enc.len() < 40 || req.wrapped_dek.len() < 40 {
        return Err(ApiError::BadRequest(
            "payload_enc and wrapped_dek must be sealed values".into(),
        ));
    }

    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO credentials (id, profile_id, label, site, payload_enc, wrapped_dek)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(profile_id)
    .bind(&req.label)
    .bind(&req.site)
    .bind(req.payload_enc.as_bytes())
    .bind(req.wrapped_dek.as_bytes())
    .execute(db.as_mut())
    .await?;

    audit(db.as_mut(), &caller, "credential.create", Some(profile_id),
          json!({ "label": req.label })).await?;
    Ok(Json(json!({ "id": id })))
}

async fn edit_credential(
    mut db: auth::Db,
    Path((profile_id, credential_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<CredentialBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::RevealSecrets) {
        return Err(ApiError::Denied(Perm::RevealSecrets));
    }
    if req.payload_enc.len() < 40 || req.wrapped_dek.len() < 40 {
        return Err(ApiError::BadRequest(
            "payload_enc and wrapped_dek must be sealed values".into(),
        ));
    }

    // profile_id in the WHERE as well as the id: without it, a caller with
    // rights on one profile could edit a login belonging to another by naming
    // its id in the path, and the permission check above would have passed on
    // the wrong profile.
    let done = sqlx::query(
        "UPDATE credentials SET label = $3, site = $4, payload_enc = $5, wrapped_dek = $6,
                updated_at = now()
         WHERE id = $1 AND profile_id = $2",
    )
    .bind(credential_id)
    .bind(profile_id)
    .bind(&req.label)
    .bind(&req.site)
    .bind(req.payload_enc.as_bytes())
    .bind(req.wrapped_dek.as_bytes())
    .execute(db.as_mut())
    .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    audit(db.as_mut(), &caller, "credential.edit", Some(profile_id),
          json!({ "label": req.label })).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_credential(
    mut db: auth::Db,
    Path((profile_id, credential_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if perms == PermSet::NONE {
        return Err(ApiError::NotFound);
    }
    if !perms.has(Perm::RevealSecrets) {
        return Err(ApiError::Denied(Perm::RevealSecrets));
    }

    // Hard delete, unlike a profile. A login somebody removed is a login they
    // meant to remove, and a soft-deleted password is a password still in the
    // database.
    let done = sqlx::query("DELETE FROM credentials WHERE id = $1 AND profile_id = $2")
        .bind(credential_id)
        .bind(profile_id)
        .execute(db.as_mut())
        .await?;
    if done.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    audit(db.as_mut(), &caller, "credential.delete", Some(profile_id),
          json!({ "id": credential_id })).await?;
    Ok(Json(json!({ "deleted": true })))
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
// They go through it STREAMED, which is not the same statement. The first
// version took `body: Bytes`, and two things followed that nobody noticed
// because the tests used bundles a few hundred bytes long:
//
//   - axum's default body limit is 2 MB. Every real profile on the machine this
//     was found on was larger — 8.4, 8.7 and 33 MB — so team sync answered 413
//     to every upload it had ever been asked to make, and said so only in a
//     status code nothing was reading.
//   - a bundle was held whole in server memory. Three operators closing large
//     profiles at once is the machine.
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
/// The largest bundle this server will accept, in bytes.
///
/// There is a limit because a body arriving over HTTP is written by whoever is
/// sending it, and "no limit" means one operator can fill the disk. There is
/// not a SMALL limit because axum's default is 2 MB and that is smaller than
/// any real profile: measured on this machine, three profiles in daily use were
/// 8.4, 8.7 and 33 MB. Team sync had never worked for a real profile — every
/// upload was refused with 413, and nothing said so out loud.
///
/// One gigabyte is generous on purpose. Bundles skip caches now (bundle.rs) so
/// what travels is cookies, storage, history and preferences, which is
/// megabytes; a profile that reaches a gigabyte has something in it worth
/// asking about, and a refusal is the way to ask.
pub fn max_bundle_bytes() -> usize {
    std::env::var("FURY_MAX_BUNDLE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1024 * 1024 * 1024)
}

async fn upload_bundle(
    State(state): State<Arc<AppState>>,
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
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
    .fetch_one(db.as_mut())
    .await?;
    if !holds_lock {
        return Err(ApiError::Conflict(
            "this profile is not locked by you — it may have been taken over, or the lock \
             lapsed while the browser was running. Nothing was overwritten"
                .into(),
        ));
    }

    // Streamed to a temporary file, hashed on the way past, and only then moved
    // into place. Three things at once:
    //
    //   - a bundle is never held whole in memory. It used to be
    //     `body: Bytes`, which meant a 300 MB upload was 300 MB of server, and
    //     several at once was the machine.
    //   - the digest is verified BEFORE anything is visible. A truncated upload
    //     recorded as a version is a profile that restores broken, and the
    //     operator finds out on the machine that needed it.
    //   - the move is atomic within the directory, so a client that disappears
    //     mid-upload leaves a stray temporary file rather than a half-written
    //     version somebody will later be handed.
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};

    let dir = bundle_root().join(profile_id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::Internal(e.into()))?;
    let staging = dir.join(format!(".incoming-{}", Uuid::now_v7()));

    // What this organisation has already stored, so the ceiling is on the org
    // and not only on the single upload. Read once, before the stream, because
    // asking per chunk would be a query per 8 KB.
    //
    // Counted from the rows rather than the filesystem: the rows are what a
    // delete removes, so freeing space frees the allowance, and a stray file
    // left by a crash does not charge somebody for storage they cannot see.
    let org_used: i64 = sqlx::query_scalar(
        // ::BIGINT is load-bearing. sum() over BIGINT returns NUMERIC in
        // PostgreSQL, and decoding that into i64 fails at runtime — so without
        // the cast, setting FURY_MAX_STORAGE_PER_ORG turns every upload into a
        // 500 and the limit breaks the thing it was meant to protect. Found by
        // uploading, not by reading.
        "SELECT coalesce(sum(b.size_bytes), 0)::BIGINT FROM bundles b
         JOIN profiles p ON p.id = b.profile_id
         WHERE p.org_id = $1",
    )
    .bind(caller.org_id)
    .fetch_one(db.as_mut())
    .await?;
    let org_ceiling = state.limits.max_storage_per_org;

    let mut hasher = Sha256::new();
    let mut written: usize = 0;
    let limit = max_bundle_bytes();
    {
        let mut file = std::fs::File::create(&staging).map_err(|e| ApiError::Internal(e.into()))?;
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                let _ = std::fs::remove_file(&staging);
                ApiError::BadRequest(format!("the upload stopped early: {e}"))
            })?;
            written += chunk.len();
            if written > limit {
                let _ = std::fs::remove_file(&staging);
                return Err(ApiError::BadRequest(format!(
                    "this bundle is over the {} limit for this server",
                    human_bytes(limit as i64)
                )));
            }
            // Stopped mid-stream rather than after it. A refusal that arrives
            // once the whole thing has been received has already cost the disk
            // and the bandwidth it was meant to protect.
            if let Some(ceiling) = org_ceiling {
                if org_used + written as i64 > ceiling {
                    let _ = std::fs::remove_file(&staging);
                    return Err(ApiError::BadRequest(format!(
                        "this organisation may store {} and has {}; the upload was \
                         stopped after {}. Delete a profile, or ask whoever runs the \
                         server for more room",
                        human_bytes(ceiling),
                        human_bytes(org_used),
                        human_bytes(written as i64),
                    )));
                }
            }
            hasher.update(&chunk);
            use std::io::Write;
            if let Err(e) = file.write_all(&chunk) {
                let _ = std::fs::remove_file(&staging);
                return Err(ApiError::Internal(e.into()));
            }
        }
    }

    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if actual != sha256 {
        let _ = std::fs::remove_file(&staging);
        return Err(ApiError::BadRequest(
            "the body does not match the digest it was sent with".into(),
        ));
    }

    let current: i32 = sqlx::query_scalar("SELECT current_version FROM profiles WHERE id = $1")
        .bind(profile_id)
        .fetch_optional(db.as_mut())
        .await?
        .ok_or(ApiError::NotFound)?;

    if base_version != current {
        return Err(ApiError::Conflict(format!(
            "this is based on version {base_version}, but the profile is at {current} —              someone else uploaded in between"
        )));
    }

    let version = current + 1;
    let key = format!("{profile_id}/{version}.bundle");
    std::fs::rename(&staging, bundle_root().join(&key))
        .map_err(|e| ApiError::Internal(e.into()))?;

    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO bundles (id, profile_id, version, kind, s3_key, size_bytes, sha256,
                              wrapped_dek, uploaded_by)
         VALUES ($1, $2, $3, 'snapshot', $4, $5, $6, $7, $8)",
    )
    .bind(Uuid::now_v7())
    .bind(profile_id)
    .bind(version)
    .bind(&key)
    .bind(written as i64)
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

    audit(db.as_mut(), &caller, "bundle.upload", Some(profile_id),
          json!({ "version": version, "bytes": written })).await?;

    Ok(Json(json!({ "version": version })))
}

/// Fetch the current version.
async fn download_bundle(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
) -> Result<axum::response::Response, ApiError> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
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
    .fetch_optional(db.as_mut())
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
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

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
    .execute(db.as_mut())
    .await?;

    if updated.rows_affected() == 0 {
        return Err(ApiError::Conflict(
            "this lock is no longer yours; stop and re-acquire before uploading".into(),
        ));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn release_lock(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    // Copied out so that reading it does not borrow the connection: a
    // handler needs both in the same expression constantly.
    let caller = db.caller;

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
        .execute(db.as_mut())
        .await?;

    // No error when nothing was deleted: the caller asked not to be holding
    // this profile, and after a force-release or a lapse they already are not.
    // Failing here would leave a client unable to tidy up after itself.
    audit(db.as_mut(), &caller, "profile.stop", Some(profile_id), json!({})).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------

/// Appends to the audit log. Failures are logged, never propagated: losing an
/// audit row must not fail the operation the user asked for, and the row is
/// append-only by grant, so it cannot be rewritten afterwards either.
/// Write an audit row on the caller's own connection.
///
/// Takes the connection rather than the pool because audit_events is under a
/// policy now: a row written from an unbound connection is refused, and one
/// written from somebody else's would carry the wrong org_id past the check.
async fn audit(
    db: &mut sqlx::PgConnection,
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
    .execute(db)
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
