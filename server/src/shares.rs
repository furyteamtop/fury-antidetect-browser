// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Handing one profile to one person, across organisations.
//!
//! # Why this exists beside projects and grants
//!
//! The server's team model is an organisation with projects inside it, and
//! access is granted on a project to a member. That is the right shape for a
//! team and the wrong shape for the thing people ask for first: "let me give
//! this profile to that person". They are not asking to be colleagues.
//!
//! # What it reuses
//!
//! Everything. `profile_grants` has carried per-profile permissions since 0001
//! and was only ever usable inside one organisation, because row-level security
//! stops the grantee's query before the grant is read. Migration 0008 widens
//! those policies to "in the organisation, OR holding a live grant", and adds
//! two columns for keys. No new permission model, no second access path.
//!
//! # The keys, and why the server never holds one
//!
//! A member opens a profile's bundle with a subkey derived from the
//! ORGANISATION key. Somebody outside that organisation has no such key, so the
//! share carries the profile's key sealed to the grantee's X25519 public key —
//! sealed by the OWNER's client, opened by the grantee's, opaque here.
//!
//! The proxy key travels the same way and is not optional in spirit. A profile
//! handed over without its proxy goes out through the receiver's own address,
//! and an account that has only ever been seen from one country appearing from
//! another is precisely the failure this product exists to prevent.
//!
//! # What revoking does
//!
//! Closes future access, deletes the grant, drops any lock the grantee holds.
//! It does not reach cookies already on their machine and nothing can. That is
//! true of every product with this feature; it is written here so the interface
//! is not tempted to imply otherwise.

use axum::extract::{Path, Query};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::audit;
use crate::error::{ApiError, ApiResult};
use crate::auth;
use crate::rbac_guard;
use fury_shared::rbac::{Perm, PermSet};

/// Somebody to share with, found by their whole address.
#[derive(Deserialize)]
pub struct LookupQuery {
    pub email: String,
}

#[derive(Serialize)]
pub struct FoundUser {
    pub user_id: Uuid,
    /// Base64. The client seals the profile key to this and sends it back.
    pub public_key: String,
}

/// Find one person by their exact address.
///
/// WHOLE address only, never a prefix and never a list. The difference matters
/// on a server anybody can sign up to: this answers "does the address I was
/// given exist", and cannot be turned into "who is on this server". A wrong
/// address and a disabled account are the same 404 for the same reason.
pub async fn lookup_user(
    mut db: auth::Db,
    Query(q): Query<LookupQuery>,
) -> ApiResult<Json<FoundUser>> {
    let email = q.email.trim().to_ascii_lowercase();
    if !email.contains('@') {
        return Err(ApiError::BadRequest("that is not an email address".into()));
    }

    let found: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
        "SELECT id, public_key FROM users \
         WHERE lower(email) = $1 AND disabled_at IS NULL",
    )
    .bind(&email)
    .fetch_optional(db.as_mut())
    .await?;

    let (user_id, public_key) = found.ok_or(ApiError::NotFound)?;
    use base64::Engine;
    Ok(Json(FoundUser {
        user_id,
        public_key: base64::engine::general_purpose::STANDARD.encode(public_key),
    }))
}

#[derive(Deserialize)]
pub struct ShareRequest {
    pub user_id: Uuid,
    pub permissions: Vec<Perm>,
    /// Base64 of the profile key, sealed to the recipient's public key.
    pub wrapped_key: String,
    /// Base64, and only when the profile has a proxy.
    pub wrapped_proxy_key: Option<String>,
    pub expires_at: Option<String>,
}

/// Give a profile to somebody.
pub async fn share_profile(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
    Json(req): Json<ShareRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;

    // ManageAccess on the profile, which is what "may give this away" means
    // everywhere else in this server. Resolved through the profile's project,
    // so a manager of that project can share and a member cannot.
    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if !perms.has(Perm::ManageAccess) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    // Sharing with yourself is not an error worth an error message, but writing
    // the row would create a grant that outlives leaving the organisation and
    // silently keeps access. Refused.
    if req.user_id == caller.user_id {
        return Err(ApiError::BadRequest(
            "that is your own account; you already have this profile".into(),
        ));
    }

    use base64::Engine;
    let b64 = |what: &str, v: &str| -> Result<Vec<u8>, ApiError> {
        base64::engine::general_purpose::STANDARD
            .decode(v)
            .map_err(|_| ApiError::BadRequest(format!("{what} is not base64")))
    };
    let wrapped_key = b64("wrapped_key", &req.wrapped_key)?;
    if wrapped_key.is_empty() {
        return Err(ApiError::BadRequest(
            "a share with no key would give access to a profile that cannot be opened".into(),
        ));
    }
    let wrapped_proxy_key = match &req.wrapped_proxy_key {
        Some(v) if !v.is_empty() => Some(b64("wrapped_proxy_key", v)?),
        _ => None,
    };

    // Capped at what the sharer holds. Somebody who cannot reveal secrets
    // cannot hand out the right to reveal them, and the ceiling is applied here
    // rather than trusted from the request.
    let requested = PermSet::from_iter(req.permissions.iter().copied());
    let capped = PermSet(requested.0 & perms.0);
    if !capped.has(Perm::View) {
        return Err(ApiError::BadRequest(
            "a share must at least allow seeing the profile".into(),
        ));
    }

    sqlx::query(
        "INSERT INTO profile_grants \
           (profile_id, user_id, permissions, granted_by, expires_at, wrapped_key, wrapped_proxy_key) \
         VALUES ($1, $2, $3, $4, $5::timestamptz, $6, $7) \
         ON CONFLICT (profile_id, user_id) DO UPDATE \
         SET permissions = EXCLUDED.permissions, granted_by = EXCLUDED.granted_by, \
             granted_at = now(), expires_at = EXCLUDED.expires_at, \
             wrapped_key = EXCLUDED.wrapped_key, \
             wrapped_proxy_key = EXCLUDED.wrapped_proxy_key",
    )
    .bind(profile_id)
    .bind(req.user_id)
    .bind(capped.0)
    .bind(caller.user_id)
    .bind(req.expires_at.as_deref())
    .bind(&wrapped_key)
    .bind(wrapped_proxy_key.as_deref())
    .execute(db.as_mut())
    .await?;

    audit(
        db.as_mut(),
        &caller,
        "profile.share",
        Some(profile_id),
        serde_json::json!({ "to": req.user_id, "permissions": capped.0 }),
    )
    .await?;

    Ok(Json(serde_json::json!({ "ok": true, "permissions": capped.0 })))
}

#[derive(Serialize)]
pub struct SharedProfile {
    pub id: Uuid,
    pub name: String,
    pub persona_id: String,
    pub tags: Vec<String>,
    /// Who gave it, so the receiver can tell two identical names apart.
    pub shared_by: String,
    pub permissions: i64,
    pub expires_at: Option<String>,
    pub wrapped_key: String,
    pub wrapped_proxy_key: Option<String>,
    pub proxy_display: Option<String>,
}

/// Everything somebody else has given to the caller.
///
/// Reachable BECAUSE of the policies in 0008 rather than in spite of them: the
/// query names no organisation, and the rows come back only where a live grant
/// exists. That is deliberate — a filter the application writes can be
/// forgotten, and this one cannot be.
pub async fn shared_with_me(mut db: auth::Db) -> ApiResult<Json<Vec<SharedProfile>>> {
    let caller = db.caller;

    // The expiry is formatted by Postgres rather than carried as a timestamp
    // type: this server has one spelling for a time on the wire and it is the
    // `rfc3339` helper in api.rs. Two spellings would eventually disagree.
    let rows: Vec<(
        Uuid,
        String,
        String,
        Vec<String>,
        String,
        i64,
        Option<String>,
        Vec<u8>,
        Option<Vec<u8>>,
        Option<String>,
        Option<i32>,
    )> = sqlx::query_as(&format!(
        "SELECT p.id, p.name, p.persona_id, p.tags, u.email::text, g.permissions, {expires}, \
                g.wrapped_key, g.wrapped_proxy_key, x.host, x.port \
         FROM profile_grants g \
         JOIN profiles p ON p.id = g.profile_id AND p.deleted_at IS NULL \
         JOIN users u ON u.id = g.granted_by \
         LEFT JOIN proxies x ON x.id = p.proxy_id AND x.deleted_at IS NULL \
         WHERE g.user_id = $1 \
           AND g.wrapped_key IS NOT NULL \
           AND (g.expires_at IS NULL OR g.expires_at > now()) \
         ORDER BY p.name",
        expires = crate::api::rfc3339("g.expires_at"),
    ))
    .bind(caller.user_id)
    .fetch_all(db.as_mut())
    .await?;

    use base64::Engine;
    let enc = base64::engine::general_purpose::STANDARD;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, name, persona_id, tags, shared_by, permissions, expires_at, wk, wpk, host, port)| {
                    SharedProfile {
                        id,
                        name,
                        persona_id,
                        tags,
                        shared_by,
                        permissions,
                        expires_at,
                        wrapped_key: enc.encode(wk),
                        wrapped_proxy_key: wpk.map(|v| enc.encode(v)),
                        proxy_display: match (host, port) {
                            (Some(h), Some(p)) => Some(format!("{h}:{p}")),
                            _ => None,
                        },
                    }
                },
            )
            .collect(),
    ))
}

/// Who currently holds a profile, for the person who gave it to them.
pub async fn list_shares(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if !perms.has(Perm::ManageAccess) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    let rows: Vec<(Uuid, String, i64, Option<String>)> = sqlx::query_as(&format!(
        "SELECT u.id, u.email::text, g.permissions, {expires} \
         FROM profile_grants g JOIN users u ON u.id = g.user_id \
         WHERE g.profile_id = $1 AND g.wrapped_key IS NOT NULL \
         ORDER BY u.email",
        expires = crate::api::rfc3339("g.expires_at"),
    ))
    .bind(profile_id)
    .fetch_all(db.as_mut())
    .await?;

    Ok(Json(serde_json::json!(rows
        .into_iter()
        .map(|(id, email, permissions, expires_at)| serde_json::json!({
            "user_id": id,
            "email": email,
            "permissions": permissions,
            "expires_at": expires_at,
        }))
        .collect::<Vec<_>>())))
}

/// Take it back.
pub async fn revoke_share(
    mut db: auth::Db,
    Path((profile_id, user_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if !perms.has(Perm::ManageAccess) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    sqlx::query("DELETE FROM profile_grants WHERE profile_id = $1 AND user_id = $2")
        .bind(profile_id)
        .bind(user_id)
        .execute(db.as_mut())
        .await?;

    // The lock goes with it, for the reason revoking project access does the
    // same: otherwise a revoked person's browser keeps the profile marked
    // in-use for as long as its heartbeat runs, and the owner cannot open a
    // profile they never stopped owning.
    sqlx::query("DELETE FROM profile_locks WHERE profile_id = $1 AND user_id = $2")
        .bind(profile_id)
        .bind(user_id)
        .execute(db.as_mut())
        .await?;

    audit(
        db.as_mut(),
        &caller,
        "profile.unshare",
        Some(profile_id),
        serde_json::json!({ "from": user_id }),
    )
    .await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// What the sharer's client needs in order to seal the keys.
///
/// One request rather than three, and it returns only what cannot be derived
/// locally: the profile key comes from the organisation key the client already
/// holds, but the PROXY's data key is wrapped under that organisation key and
/// lives on the server. Without it a share would go out with no proxy, and a
/// profile that goes out through the receiver's own address is the failure this
/// product exists to prevent -- so it is fetched rather than skipped.
///
/// Behind ManageAccess, the same right that sharing needs. It hands over a
/// wrapped key, and a wrapped key is only useful to somebody who already has
/// the organisation key.
pub async fn share_material(
    mut db: auth::Db,
    Path(profile_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller = db.caller;
    let perms = rbac_guard::permissions_for_profile(db.as_mut(), caller.user_id, profile_id).await?;
    if !perms.has(Perm::ManageAccess) {
        return Err(ApiError::Denied(Perm::ManageAccess));
    }

    let row: Option<(Option<Uuid>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT p.proxy_id, x.wrapped_dek \
         FROM profiles p LEFT JOIN proxies x ON x.id = p.proxy_id AND x.deleted_at IS NULL \
         WHERE p.id = $1 AND p.deleted_at IS NULL",
    )
    .bind(profile_id)
    .fetch_optional(db.as_mut())
    .await?;

    let (proxy_id, wrapped_dek) = row.ok_or(ApiError::NotFound)?;
    // Hex, not base64. A wrapped data key is spelled in hex everywhere else
    // this pair talks about proxies -- rotation_material, edit_proxy -- and a
    // second spelling for the same thing is a decoding bug waiting for the day
    // somebody copies the wrong helper.
    let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Ok(Json(serde_json::json!({
        "proxy_id": proxy_id,
        "wrapped_proxy_dek": wrapped_dek.map(|v| hex(&v)),
    })))
}
