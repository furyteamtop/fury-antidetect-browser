//! Fury server — the self-hosted coordination backend.
//!
//! Deliberately dumb. It does not generate fingerprints, does not check proxies,
//! and cannot decrypt a bundle: it stores ciphertext plus metadata, resolves
//! permissions, and hands out presigned URLs. That is what lets it run on the
//! cheapest VPS available and makes its compromise survivable.

mod api;
mod auth;
mod enroll;
mod error;
mod rbac_guard;

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
    /// it. What they cost you is rows.
    pub open_signup: bool,
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
    if open_signup {
        tracing::warn!("FURY_OPEN_SIGNUP is on: anyone who can reach this server can make an account");
    }
    let state = Arc::new(AppState { db, open_signup });
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
        .connect(&database_url)
        .await?)
}
