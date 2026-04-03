pub mod config;
pub mod error;
pub mod auth;
pub mod wallet;
pub mod treasury;
pub mod stellar;
pub mod horizon;
pub mod resync;
pub mod constitution;
pub mod proposal;
pub mod agent;
pub mod policy;
pub mod permit;
pub mod dashboard;
pub mod multisig;

use redis::aio::ConnectionManager;
use sqlx::PgPool;
use uuid::Uuid;
use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TreasuryEvent {
    WalletBalanceUpdate {
        treasury_id: Uuid,
        wallet_address: String,
        amount: f64,
        asset_code: String,
    },
    ConstitutionUpdate {
        treasury_id: Uuid,
        version: i32,
    },
    PermitIssued {
        treasury_id: Uuid,
        agent_id: Uuid,
        permit_id: Uuid,
        wallet_address: String,
        approved_amount: f64,
    },
    PermitConsumed {
        treasury_id: Uuid,
        permit_id: Uuid,
        wallet_address: String,
    },
    PermitExpired {
        treasury_id: Uuid,
        permit_id: Uuid,
        wallet_address: String,
    },
    TreasuryHalted {
        treasury_id: Uuid,
    },
    TreasuryResumed {
        treasury_id: Uuid,
    },
    AgentSuspended {
        treasury_id: Uuid,
        agent_id: Uuid,
    },
    AgentStatusChanged {
        treasury_id: Uuid,
        agent_id: Uuid,
        new_status: String,
    },
    AgentConnected {
        treasury_id: Uuid,
        agent_id: Uuid,
    },
    AgentSignerAdded {
        treasury_id: Uuid,
        agent_id: Uuid,
        wallet_address: String,
        tx_hash: String,
    },
    AgentActivated {
        treasury_id: Uuid,
        agent_id: Uuid,
    },
}

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub redis: ConnectionManager,
    pub config: config::Settings,
    pub tx_events: tokio::sync::broadcast::Sender<TreasuryEvent>,
}

use axum::{routing::get, Router};

pub fn router(state: AppState) -> Router {
    let treasury_v1 = Router::new()
        .merge(treasury::router())
        .merge(constitution::router())
        .merge(proposal::router());

    Router::new()
        .route("/", get(root))
        .nest("/v1/auth", auth::router())
        .nest("/v1/treasuries", treasury_v1)
        .nest("/v1/wallets", wallet::router())
        .nest("/v1/agents", agent::router())
        .nest("/v1/multisig", multisig::router())
        .nest("/v1/permits", permit::router())
        .nest("/v1/dashboard", dashboard::router())
        .nest("/admin", resync::router())
        .with_state(state)
}

async fn root() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
