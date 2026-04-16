use axum::{
    extract::{Path, State, ws::{WebSocket, WebSocketUpgrade, Message}},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;
use crate::error::{AppResult, AppError};
use crate::AppState;
use crate::auth::AuthUser;
use crate::resync::WalletBalance;
use sqlx::Row;

async fn user_owns_treasury(state: &AppState, user_id: Uuid, treasury_id: Uuid) -> AppResult<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM treasuries WHERE treasury_id = $1 AND owner_user_id = $2)"
    )
    .bind(treasury_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .map_err(Into::into)
}

async fn owned_treasury_ids(state: &AppState, user_id: Uuid) -> AppResult<HashSet<Uuid>> {
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT treasury_id FROM treasuries WHERE owner_user_id = $1"
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(ids.into_iter().collect())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TreasurySummary {
    pub treasury_id: Uuid,
    pub name: String,
    pub health: String,
    pub current_aum_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct DashboardTreasuryState {
    pub treasury_id: Uuid,
    pub name: String,
    pub network: String,
    pub health: String,
    pub current_aum_usd: f64,
    pub peak_aum_usd: f64,
    pub constitution_version: i32,
    pub active_permit_count: i64,
    pub max_drawdown_pct: f64,
    pub drawdown_current_pct: f64,
    pub wallets: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct DashboardBalances {
    pub treasury_id: Uuid,
    pub balances: Vec<WalletBalance>,
    pub total_aum_usd: f64,
}

pub async fn list_treasuries(
    State(state): State<AppState>,
    auth: AuthUser,
) -> AppResult<Json<Vec<TreasurySummary>>> {
    let rows = sqlx::query(
        "SELECT treasury_id, name, health, current_aum_usd::float8 FROM treasuries WHERE owner_user_id = $1"
    )
    .bind(auth.user_id)
    .fetch_all(&state.db)
    .await?;

    let summaries = rows.into_iter().map(|r| TreasurySummary {
        treasury_id: r.get(0),
        name: r.get(1),
        health: r.get(2),
        current_aum_usd: r.get(3),
    }).collect();

    Ok(Json(summaries))
}

pub async fn get_treasury_state(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<DashboardTreasuryState>> {
    let row = sqlx::query(
        "SELECT t.treasury_id, t.name, t.network, t.health, t.current_aum_usd::float8, t.peak_aum_usd::float8, t.constitution_version, c.content
         FROM treasuries t
         LEFT JOIN constitution_history c ON c.treasury_id = t.treasury_id AND c.version = t.constitution_version
         WHERE t.treasury_id = $1 AND t.owner_user_id = $2"
    )
    .bind(id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::TreasuryNotFound)?;

    // Active permit count
    let active_permit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM permits WHERE treasury_id = $1 AND status = 'ACTIVE'"
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    // Drawdown calculation
    let peak_aum: f64 = row.get(5);
    let current_aum: f64 = row.get(4);
    let drawdown_current_pct = if peak_aum > 0.0 {
        ((peak_aum - current_aum) / peak_aum * 100.0).max(0.0)
    } else {
        0.0
    };

    // Max drawdown from constitution
    let max_drawdown_pct = row.get::<Option<serde_json::Value>, _>(7)
        .and_then(|v| v.get("treasury_rules").and_then(|tr| tr.get("max_drawdown_pct")).and_then(|d| d.as_f64()))
        .unwrap_or(20.0);

    let wallets = sqlx::query(
        "SELECT wallet_address, label, multisig_active, status FROM treasury_wallets WHERE treasury_id = $1"
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let wallets_json = serde_json::to_value(wallets.into_iter().map(|r| {
        serde_json::json!({
            "wallet_address": r.get::<String, _>(0),
            "label": r.get::<Option<String>, _>(1),
            "multisig_active": r.get::<bool, _>(2),
            "status": r.get::<String, _>(3),
        })
    }).collect::<Vec<_>>()).unwrap_or(serde_json::Value::Array(vec![]));

    Ok(Json(DashboardTreasuryState {
        treasury_id: row.get(0),
        name: row.get(1),
        network: row.get(2),
        health: row.get(3),
        current_aum_usd: current_aum,
        peak_aum_usd: peak_aum,
        constitution_version: row.get(6),
        active_permit_count,
        max_drawdown_pct,
        drawdown_current_pct,
        wallets: wallets_json,
    }))
}

/// Return cached balance snapshot for a treasury's wallets
pub async fn get_treasury_balances(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<DashboardBalances>> {
    use redis::AsyncCommands;

    if !user_owns_treasury(&state, auth.user_id, id).await? {
        return Err(AppError::TreasuryNotFound);
    }

    let mut redis = state.redis.clone();

    let snapshot_key = format!("treasury:snapshot:{}", id);
    let cached: Option<String> = redis.get(&snapshot_key).await.unwrap_or(None);

    let (balances, total_aum) = if let Some(json_str) = cached {
        let balances: Vec<WalletBalance> = serde_json::from_str(&json_str).unwrap_or_default();
        let total: f64 = balances.iter().map(|b| b.usd_value).sum();
        (balances, total)
    } else {
        // Trigger a fresh resync if no cached data
        let result = crate::resync::resync_treasury(id, &state).await?;
        (result.balances, result.total_aum_usd)
    };

    Ok(Json(DashboardBalances {
        treasury_id: id,
        balances,
        total_aum_usd: total_aum,
    }))
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    auth: AuthUser,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, auth))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, auth: AuthUser) {
    let mut rx = state.tx_events.subscribe();
    let mut allowed_treasuries = owned_treasury_ids(&state, auth.user_id)
        .await
        .unwrap_or_default();
    
    while let Ok(event) = rx.recv().await {
        let treasury_id = event.treasury_id();

        if !allowed_treasuries.contains(&treasury_id) {
            match user_owns_treasury(&state, auth.user_id, treasury_id).await {
                Ok(true) => {
                    allowed_treasuries.insert(treasury_id);
                }
                Ok(false) => continue,
                Err(_) => continue,
            }
        }

        let envelope = event.to_envelope();
        let msg = serde_json::to_string(&envelope).unwrap();
        if socket.send(Message::Text(msg)).await.is_err() {
            break;
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_treasuries))
        .route("/:id", get(get_treasury_state))
        .route("/:id/balances", get(get_treasury_balances))
        .route("/ws", get(ws_handler))
}
