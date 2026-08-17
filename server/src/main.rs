// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Fury server — the self-hosted coordination backend.
//!
//! Deliberately dumb. It does not generate fingerprints, does not check proxies,
//! and cannot decrypt a bundle: it stores ciphertext plus metadata, resolves
//! permissions, and hands out presigned URLs. That is what lets it run on the
//! cheapest VPS available and makes its compromise survivable.

mod shares;
mod api;
mod auth;
mod enroll;
mod error;
mod rbac_guard;
#[cfg(test)]
mod rls_tests;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    /// Whether a stranger may make themselves an account.
    ///
    /// Off unless `FURY_OPEN_SIGNUP=1`. A self-hosted server is somebody's own
    /// machine, and the default for "may anyone who finds this address join"
    /// is no. Turning it on is what makes a server a service.
    ///
    /// It is not as dangerous as it sounds: every sign-up creates its OWN
    /// organisation, so a stranger who joins can see nothing of anyone else's
    /// — the organisation is the boundary, and there is no way to reach across
    /// it. What they cost you is rows — which is what `limits` is about.
    pub open_signup: bool,
    /// What one organisation may take from a shared server.
    pub limits: Limits,
}

/// Ceilings, per organisation and per server.
///
/// Every one of them is off unless set, and that is right for the ordinary
/// deployment: a box you run for your own team does not need a quota, and a
/// limit somebody hits at nine on a Monday is worse than no limit at all.
///
/// They exist for the other deployment. `FURY_OPEN_SIGNUP=1` makes the server a
/// service anybody who knows the address can join, and the isolation between
/// organisations is total — nobody can read anybody's data — but isolation is
/// not the same as fairness. Without a ceiling, one stranger with a script owns
/// as much of your disk as they care to take, and the first you hear of it is
/// the database refusing writes for everyone.
///
/// So: turn them on when you open sign-ups, leave them off when you do not.
#[derive(Clone, Copy, Default)]
pub struct Limits {
    /// Organisations on this server, total. Refuses further sign-ups.
    ///
    /// The blunt one, and the one that matters most: an organisation is what a
    /// sign-up creates, so this is the tap. Existing members can still be
    /// invited into the organisations that exist.
    pub max_orgs: Option<i64>,
    /// Profiles one organisation may hold, excluding deleted ones.
    pub max_profiles_per_org: Option<i64>,
    /// Bundle bytes one organisation may store.
    ///
    /// Counted over the versions actually on disk, so deleting a profile or
    /// pruning old versions frees the allowance rather than leaving a
    /// high-water mark somebody has to ask about.
    pub max_storage_per_org: Option<i64>,
}

impl Limits {
    fn from_env() -> Self {
        let n = |key: &str| {
            std::env::var(key).ok().and_then(|v| v.trim().parse::<i64>().ok()).filter(|v| *v > 0)
        };
        Self {
            max_orgs: n("FURY_MAX_ORGS"),
            max_profiles_per_org: n("FURY_MAX_PROFILES_PER_ORG"),
            max_storage_per_org: n("FURY_MAX_STORAGE_PER_ORG"),
        }
    }

    fn describe(&self) -> String {
        let one = |label: &str, v: Option<i64>| match v {
            Some(v) => format!("{label}={v}"),
            None => format!("{label}=unlimited"),
        };
        format!(
            "{}, {}, {}",
            one("orgs", self.max_orgs),
            one("profiles/org", self.max_profiles_per_org),
            one("storage/org", self.max_storage_per_org),
        )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fury_server=info,tower_http=info".into()),
        )
        .init();

    // One subcommand, and it is the one a self-hoster needs before the server is
    // any use to them: there is no registration form, so the first account is
    // invited from the shell of the machine running the database.
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("invite") => return enroll::cli(&args[1..]).await,
        Some(other) => anyhow::bail!("unknown command: {other}\n\nusage:\n  fury-server\n  fury-server invite --email you@example.com --org \"My team\""),
        None => {}
    }

    let bind: SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()?;

    // Before the socket, not at the first upload.
    let bundles = api::check_bundle_root()?;
    tracing::info!(dir = %bundles.display(), "bundles");

    let db = connect().await?;
    sqlx::migrate!("./migrations").run(&db).await?;

    // Anything but "1"/"true" is off, including a typo. A setting that opens
    // the door should not be openable by accident.
    let open_signup = std::env::var("FURY_OPEN_SIGNUP")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let limits = Limits::from_env();
    if open_signup {
        tracing::warn!(
            limits = %limits.describe(),
            "FURY_OPEN_SIGNUP is on: anyone who can reach this server can make an account"
        );
        if limits.max_orgs.is_none() {
            // Said separately and loudly. Isolation between organisations is
            // total; fairness is not, and the failure mode of an open server
            // with no ceiling is the database refusing writes for everybody
            // because one stranger ran a loop.
            tracing::warn!(
                "no FURY_MAX_ORGS is set, so anyone may create organisations without limit —                  see docs/13"
            );
        }
    } else {
        tracing::info!(limits = %limits.describe(), "sign-ups are by invitation only");
    }
    let state = Arc::new(AppState { db, open_signup, limits });
    let app = Router::new()
        .route("/healthz", get(healthz))
        .merge(api::routes())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    tracing::info!(%bind, "fury-server listening");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// The database, wherever `DATABASE_URL` says. Shared by the server and the
/// `invite` command so a self-hoster configures one thing, not two.
pub async fn connect() -> anyhow::Result<PgPool> {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://fury:fury@localhost/fury".to_string());
    Ok(PgPoolOptions::new()
        .max_connections(16)
        // Clear the per-request identity before a connection goes back in the
        // pool. `auth::Db` sets app.user_id at session scope rather than
        // transaction scope, because most handlers are a single statement and
        // wrapping every one in a transaction to hold a SET LOCAL would be a
        // large change for no gain. Session scope means the value outlives the
        // request unless something removes it, and the next request to borrow
        // that connection would run as whoever held it last.
        //
        // Db sets it on every acquire, so this is the second of two locks on
        // the same door. It is here because the failure it prevents — one
        // user's request reading another user's rows — is the exact failure RLS
        // exists to stop, and a defence that depends on one line never being
        // deleted is not a defence.
        .after_release(|conn, _meta| {
            Box::pin(async move {
                sqlx::query("SELECT set_config('app.user_id', '', false)")
                    .execute(conn)
                    .await?;
                Ok(true)
            })
        })
        .connect(&database_url)
        .await?)
}
