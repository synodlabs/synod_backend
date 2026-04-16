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
pub mod mcp;

use redis::aio::ConnectionManager;
use sqlx::PgPool;
use uuid::Uuid;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Unified event envelope sent over WebSocket to all consumers.
/// Every event has an `event_type` in SCREAMING_SNAKE_CASE and a `payload` object.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_type: String,
    pub payload: serde_json::Value,
}

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
    IntentConfirmed {
        treasury_id: Uuid,
        agent_id: Uuid,
        intent_id: Uuid,
        tx_hash: Option<String>,
    },
    IntentRejected {
        treasury_id: Uuid,
        agent_id: Uuid,
        intent_id: Uuid,
        reason: String,
    },
    IntentFailed {
        treasury_id: Uuid,
        agent_id: Uuid,
        intent_id: Uuid,
        reason: String,
    },
}

impl TreasuryEvent {
    pub fn treasury_id(&self) -> Uuid {
        match self {
            TreasuryEvent::WalletBalanceUpdate { treasury_id, .. }
            | TreasuryEvent::ConstitutionUpdate { treasury_id, .. }
            | TreasuryEvent::PermitIssued { treasury_id, .. }
            | TreasuryEvent::PermitConsumed { treasury_id, .. }
            | TreasuryEvent::PermitExpired { treasury_id, .. }
            | TreasuryEvent::TreasuryHalted { treasury_id }
            | TreasuryEvent::TreasuryResumed { treasury_id }
            | TreasuryEvent::AgentSuspended { treasury_id, .. }
            | TreasuryEvent::AgentStatusChanged { treasury_id, .. }
            | TreasuryEvent::AgentConnected { treasury_id, .. }
            | TreasuryEvent::AgentSignerAdded { treasury_id, .. }
            | TreasuryEvent::AgentActivated { treasury_id, .. }
            | TreasuryEvent::IntentConfirmed { treasury_id, .. }
            | TreasuryEvent::IntentRejected { treasury_id, .. }
            | TreasuryEvent::IntentFailed { treasury_id, .. } => *treasury_id,
        }
    }

    /// Convert to a unified EventEnvelope with SCREAMING_SNAKE event_type.
    pub fn to_envelope(&self) -> EventEnvelope {
        let (event_type, payload) = match self {
            TreasuryEvent::WalletBalanceUpdate { treasury_id, wallet_address, amount, asset_code } => (
                "WALLET_BALANCE_UPDATE",
                serde_json::json!({ "treasury_id": treasury_id, "wallet_address": wallet_address, "amount": amount, "asset_code": asset_code }),
            ),
            TreasuryEvent::ConstitutionUpdate { treasury_id, version } => (
                "CONSTITUTION_UPDATE",
                serde_json::json!({ "treasury_id": treasury_id, "version": version }),
            ),
            TreasuryEvent::PermitIssued { treasury_id, agent_id, permit_id, wallet_address, approved_amount } => (
                "PERMIT_ISSUED",
                serde_json::json!({ "treasury_id": treasury_id, "agent_id": agent_id, "permit_id": permit_id, "wallet_address": wallet_address, "approved_amount": approved_amount }),
            ),
            TreasuryEvent::PermitConsumed { treasury_id, permit_id, wallet_address } => (
                "PERMIT_CONSUMED",
                serde_json::json!({ "treasury_id": treasury_id, "permit_id": permit_id, "wallet_address": wallet_address }),
            ),
            TreasuryEvent::PermitExpired { treasury_id, permit_id, wallet_address } => (
                "PERMIT_EXPIRED",
                serde_json::json!({ "treasury_id": treasury_id, "permit_id": permit_id, "wallet_address": wallet_address }),
            ),
            TreasuryEvent::TreasuryHalted { treasury_id } => (
                "TREASURY_HALTED",
                serde_json::json!({ "treasury_id": treasury_id }),
            ),
            TreasuryEvent::TreasuryResumed { treasury_id } => (
                "TREASURY_RESUMED",
                serde_json::json!({ "treasury_id": treasury_id }),
            ),
            TreasuryEvent::AgentSuspended { treasury_id, agent_id } => (
                "AGENT_SUSPENDED",
                serde_json::json!({ "treasury_id": treasury_id, "agent_id": agent_id }),
            ),
            TreasuryEvent::AgentStatusChanged { treasury_id, agent_id, new_status } => (
                "AGENT_STATUS_CHANGED",
                serde_json::json!({ "treasury_id": treasury_id, "agent_id": agent_id, "new_status": new_status }),
            ),
            TreasuryEvent::AgentConnected { treasury_id, agent_id } => (
                "AGENT_CONNECTED",
                serde_json::json!({ "treasury_id": treasury_id, "agent_id": agent_id }),
            ),
            TreasuryEvent::AgentSignerAdded { treasury_id, agent_id, wallet_address, tx_hash } => (
                "AGENT_SIGNER_ADDED",
                serde_json::json!({ "treasury_id": treasury_id, "agent_id": agent_id, "wallet_address": wallet_address, "tx_hash": tx_hash }),
            ),
            TreasuryEvent::AgentActivated { treasury_id, agent_id } => (
                "AGENT_ACTIVATED",
                serde_json::json!({ "treasury_id": treasury_id, "agent_id": agent_id }),
            ),
            TreasuryEvent::IntentConfirmed { treasury_id, agent_id, intent_id, tx_hash } => (
                "INTENT_CONFIRMED",
                serde_json::json!({ "treasury_id": treasury_id, "agent_id": agent_id, "intent_id": intent_id, "tx_hash": tx_hash }),
            ),
            TreasuryEvent::IntentRejected { treasury_id, agent_id, intent_id, reason } => (
                "INTENT_REJECTED",
                serde_json::json!({ "treasury_id": treasury_id, "agent_id": agent_id, "intent_id": intent_id, "reason": reason }),
            ),
            TreasuryEvent::IntentFailed { treasury_id, agent_id, intent_id, reason } => (
                "INTENT_FAILED",
                serde_json::json!({ "treasury_id": treasury_id, "agent_id": agent_id, "intent_id": intent_id, "reason": reason }),
            ),
        };

        EventEnvelope {
            event_type: event_type.to_string(),
            payload,
        }
    }
}

/// Shared map of active Horizon watcher task handles, keyed by wallet address.
pub type WatcherHandles = Arc<Mutex<HashMap<String, JoinHandle<()>>>>;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub redis: ConnectionManager,
    pub config: config::Settings,
    pub tx_events: tokio::sync::broadcast::Sender<TreasuryEvent>,
    pub watcher_handles: WatcherHandles,
}

use axum::{routing::{get, post}, Router};

pub fn router(state: AppState) -> Router {
    let treasury_v1 = Router::new()
        .merge(treasury::router())
        .merge(constitution::router())
        .merge(proposal::router())
        .route("/:id/resync", post(resync::manual_resync));

    Router::new()
        .route("/", get(root))
        .merge(mcp::router())
        .nest("/v1/auth", auth::router())
        .nest("/v1/treasuries", treasury_v1)
        .nest("/v1/wallets", wallet::router())
        .nest("/v1/agents", agent::router())
        .nest("/v1/multisig", multisig::router())
        .nest("/v1/permits", permit::router())
        .nest("/v1/dashboard", dashboard::router())
        .nest("/admin", resync::admin_router())
        .with_state(state)
}

async fn root() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
