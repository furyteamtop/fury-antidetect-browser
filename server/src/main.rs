//! Fury server — the self-hosted coordination backend.
//!
//! Deliberately dumb. It does not generate fingerprints, does not check proxies,
//! and cannot decrypt a bundle: it stores ciphertext plus metadata, resolves
//! permissions, and hands out presigned URLs. That is what lets it run on the
//! cheapest VPS available and makes its compromise survivable.

mod api;
mod auth;
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fury_server=info,tower_http=info".into()),
        )
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://fury:fury@localhost/fury".to_string());
    let bind: SocketAddr = std::env::var("BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
        .parse()?;

    let db = PgPoolOptions::new()
        .max_connections(16)
        .connect(&database_url)
        .await?;

    sqlx::migrate!("./migrations").run(&db).await?;

    let state = Arc::new(AppState { db });
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
