// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! API errors and how they reach the client.
//!
//! One rule governs the mapping: **a caller who may not see a project is told
//! it does not exist, not that they are forbidden.** Returning 403 for a
//! resource that exists and 404 for one that does not turns any endpoint into
//! an enumeration oracle — someone with access to one project could discover
//! how many others an organisation has by walking IDs.
//!
//! So `Denied` (the caller can see the project but lacks the specific right) is
//! a 403 and says which right is missing, while `NotFound` covers both "no such
//! project" and "not yours", indistinguishably.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use fury_shared::rbac::Perm;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("authentication required")]
    Unauthenticated,

    /// The caller can see the resource but lacks this specific permission.
    #[error("missing permission")]
    Denied(Perm),

    /// Either it does not exist or the caller may not see it. Deliberately
    /// indistinguishable — see the module comment.
    #[error("not found")]
    NotFound,

    #[error("{0}")]
    BadRequest(String),

    /// Someone else holds the profile lock.
    #[error("profile is in use")]
    Locked(serde_json::Value),

    /// An upload based on a version that is no longer current.
    #[error("stale base version")]
    Conflict(String),

    #[error(transparent)]
    Db(#[from] sqlx::Error),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = match &self {
            ApiError::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                json!({ "error": "unauthenticated" }),
            ),
            ApiError::Denied(perm) => (
                StatusCode::FORBIDDEN,
                json!({ "error": "denied", "missing_permission": perm }),
            ),
            ApiError::NotFound => (StatusCode::NOT_FOUND, json!({ "error": "not_found" })),
            ApiError::BadRequest(message) => (
                StatusCode::BAD_REQUEST,
                json!({ "error": "bad_request", "message": message }),
            ),
            ApiError::Locked(holder) => (
                StatusCode::CONFLICT,
                json!({ "error": "locked", "holder": holder }),
            ),
            ApiError::Conflict(message) => (
                StatusCode::CONFLICT,
                json!({ "error": "conflict", "message": message }),
            ),
            // Internal failures never describe themselves to the client: the
            // text would leak schema and query shape. Logged in full instead.
            ApiError::Db(e) => {
                tracing::error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "internal" }),
                )
            }
            ApiError::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "internal" }),
                )
            }
        };
        (status, Json(body)).into_response()
    }
}

/// GuardError carries the same distinction the HTTP layer needs, so the mapping
/// is mechanical — and keeping it in one place means a handler cannot
/// accidentally turn an invisible project into a 403.
impl From<crate::rbac_guard::GuardError> for ApiError {
    fn from(e: crate::rbac_guard::GuardError) -> Self {
        use crate::rbac_guard::GuardError;
        match e {
            GuardError::Denied(perm) => ApiError::Denied(perm),
            // Both of these mean "you cannot see it", and the client must not be
            // able to tell them apart.
            GuardError::NoSuchProject | GuardError::NotAMember => ApiError::NotFound,
            GuardError::Db(e) => ApiError::Db(e),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    fn status_of(e: ApiError) -> StatusCode {
        e.into_response().status()
    }

    #[test]
    fn invisible_resources_are_404_not_403() {
        // The whole point: a 403 here would confirm the project exists.
        assert_eq!(status_of(ApiError::NotFound), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_missing_right_on_a_visible_project_is_403() {
        assert_eq!(
            status_of(ApiError::Denied(Perm::ExportCookies)),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn lock_contention_is_409_not_403() {
        // The caller is allowed to launch; someone else got there first. A 403
        // would send them to check their permissions instead of asking a
        // colleague to close the profile.
        assert_eq!(
            status_of(ApiError::Locked(json!({}))),
            StatusCode::CONFLICT
        );
    }

    #[test]
    fn internal_errors_do_not_describe_themselves() {
        let e = ApiError::Internal(anyhow::anyhow!("table users_secret does not exist"));
        assert_eq!(status_of(e), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
