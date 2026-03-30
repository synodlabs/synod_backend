pub mod config;
pub mod error;
pub mod auth;
pub mod wallet;
pub mod treasury;
pub mod stellar;

use redis::aio::ConnectionManager;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub redis: ConnectionManager,
    pub config: config::Settings,
}

use axum::{routing::get, Router};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .nest("/v1/auth", auth::router())
        .nest("/v1/treasuries", treasury::router())
        .nest("/v1/wallets", wallet::router())
        .with_state(state)
}

async fn root() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
