// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Resolving what a user may do, against the database.
//!
//! Two rules that the rest of the server relies on:
//!
//! 1. Permissions are resolved here and nowhere else. A handler asks
//!    `require(&db, user, project, Perm::X)` and either proceeds or returns 403.
//! 2. The answer is also sent to the client (`ProjectSummary::permissions`) so
//!    the UI can hide what is unavailable — but the UI hiding a button is
//!    presentation, never enforcement.

use fury_shared::rbac::{effective, OrgRole, Perm, PermSet};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("not a member of this organisation")]
    NotAMember,
    #[error("project not found")]
    NoSuchProject,
    #[error("missing permission: {0:?}")]
    Denied(Perm),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Effective permissions for `user` on `project`.
///
/// Returns `PermSet::NONE` rather than an error when the user is simply not
/// granted anything — callers turn that into 404, not 403, so the existence of
/// a project is not leaked to someone who cannot see it.
pub async fn permissions_for(
    db: &PgPool,
    user_id: Uuid,
    project_id: Uuid,
) -> Result<PermSet, GuardError> {
    let row: Option<(String, Option<i64>)> = sqlx::query_as(
        r#"
        SELECT m.role::text, g.permissions
        FROM projects p
        JOIN org_members m
          ON m.org_id = p.org_id AND m.user_id = $1
        LEFT JOIN project_grants g
          ON g.project_id = p.id
         AND g.user_id = $1
         AND (g.expires_at IS NULL OR g.expires_at > now())
        WHERE p.id = $2 AND p.deleted_at IS NULL
        "#,
    )
    .bind(user_id)
    .bind(project_id)
    .fetch_optional(db)
    .await?;

    let Some((role, grant)) = row else {
        // Either no such project, or the user is not in its org. Same answer
        // either way — do not distinguish, it is an enumeration oracle.
        return Ok(PermSet::NONE);
    };

    let role = parse_role(&role).ok_or(GuardError::NotAMember)?;
    Ok(effective(role, grant.map(PermSet)))
}

/// Same, resolved through the profile's project.
pub async fn permissions_for_profile(
    db: &PgPool,
    user_id: Uuid,
    profile_id: Uuid,
) -> Result<PermSet, GuardError> {
    let project: Option<(Uuid,)> =
        sqlx::query_as("SELECT project_id FROM profiles WHERE id = $1 AND deleted_at IS NULL")
            .bind(profile_id)
            .fetch_optional(db)
            .await?;

    let Some((project_id,)) = project else {
        return Ok(PermSet::NONE);
    };

    let from_project = permissions_for(db, user_id, project_id).await?;

    // A direct profile grant can only add to what the project already allows,
    // and is still capped by the org role ceiling inside `effective`.
    let direct: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT permissions FROM profile_grants
        WHERE profile_id = $1 AND user_id = $2
          AND (expires_at IS NULL OR expires_at > now())
        "#,
    )
    .bind(profile_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;

    Ok(match direct {
        Some((bits,)) => PermSet(from_project.0 | bits),
        None => from_project,
    })
}

pub async fn require(
    db: &PgPool,
    user_id: Uuid,
    project_id: Uuid,
    perm: Perm,
) -> Result<PermSet, GuardError> {
    let perms = permissions_for(db, user_id, project_id).await?;
    if perms == PermSet::NONE {
        return Err(GuardError::NoSuchProject);
    }
    if !perms.has(perm) {
        return Err(GuardError::Denied(perm));
    }
    Ok(perms)
}

fn parse_role(s: &str) -> Option<OrgRole> {
    Some(match s {
        "owner" => OrgRole::Owner,
        "admin" => OrgRole::Admin,
        "manager" => OrgRole::Manager,
        "member" => OrgRole::Member,
        _ => return None,
    })
}

/// Bind the request's user to the connection so PostgreSQL row-level security
/// can act as a second line of defence under this module.
///
/// `set_config(..., true)` scopes the setting to the transaction, so a pooled
/// connection cannot carry one request's identity into the next — which is also
/// why this cannot simply be called somewhere central and forgotten.
///
/// STILL NOT WIRED IN, and the reason is bigger than an oversight. Audited
/// 02.08.2026:
///
///   1. Six of the forty-two handlers in api.rs open a transaction. The other
///      thirty-six query the pool directly, and a transaction-local setting has
///      nowhere to live on those. Calling this in the six would enable RLS for
///      them and leave the rest reading with `app.user_id` unset — which under
///      the policies in 0001_init.sql means reading nothing at all. A partial
///      wiring is worse than none.
///
///   2. The migration's own comment says "the app connects as `fury_app`". It
///      does not. deploy/server-install.sh:83-87 creates role `fury` and gives
///      it the database with `createdb -O fury`, so the app connects as the
///      OWNER of every table — and PostgreSQL exempts a table's owner from its
///      policies unless the table is set to FORCE ROW LEVEL SECURITY, which
///      nothing does. So even with the setting bound, every policy here is
///      inert.
///
/// Both have to change together: FORCE on the six tables, and every handler
/// through a transaction that binds the user. Neither is hard; the second is
/// thirty-six handlers, and doing it half-way would take the server down rather
/// than leave it as it is.
///
/// What this does NOT mean: the per-handler RBAC above is real, it runs on
/// every request, and it is what is actually enforcing access today. RLS is the
/// second line, and the second line is missing.
#[allow(dead_code)]
pub async fn bind_rls_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.user_id', $1, true)")
        .bind(user_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_strings_match_the_pg_enum() {
        for s in ["owner", "admin", "manager", "member"] {
            assert!(parse_role(s).is_some(), "{s} must match org_role in 0001_init.sql");
        }
        assert!(parse_role("superuser").is_none());
    }
}
