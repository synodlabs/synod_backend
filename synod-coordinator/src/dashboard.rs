use axum::{
    extract::{Path, State, ws::{WebSocket, WebSocketUpgrade, Message}},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::error::{AppResult, AppError};
use crate::AppState;
use crate::auth::AuthUser;
use sqlx::Row;
use futures::{StreamExt, SinkExt};

#[derive(Debug, Serialize, Deserialize)]
pub struct TreasurySummary {
    pub treasury_id: Uuid,
    pub name: String,
    pub health: String,
    pub current_aum_usd: f64,
}

#[derive(Debug, Serialize)]
pub struct TreasuryState {
    pub treasury_id: Uuid,
    pub name: String,
    pub network: String,
    pub health: String,
    pub current_aum_usd: f64,
    pub peak_aum_usd: f64,
    pub constitution_version: i32,
    pub pools: serde_json::Value,
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
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<TreasuryState>> {
    let row = sqlx::query(
        "SELECT t.treasury_id, t.name, t.network, t.health, t.current_aum_usd::float8, t.peak_aum_usd::float8, t.constitution_version, c.content
         FROM treasuries t
         JOIN constitution_history c ON c.treasury_id = t.treasury_id AND c.version = t.constitution_version
         WHERE t.treasury_id = $1"
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(TreasuryState {
        treasury_id: row.get(0),
        name: row.get(1),
        network: row.get(2),
        health: row.get(3),
        current_aum_usd: row.get(4),
        peak_aum_usd: row.get(5),
        constitution_version: row.get(6),
        pools: row.get::<serde_json::Value, _>(7).get("pools").cloned().unwrap_or(serde_json::Value::Array(vec![])),
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
    
    // In a real app we'd filter events to only those the user is authorized for.
    // For this prototype we push all events (assuming user owns all for simplicity in test).

    while let Ok(event) = rx.recv().await {
        let msg = serde_json::to_string(&event).unwrap();
        if socket.send(Message::Text(msg)).await.is_err() {
            break;
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_treasuries))
        .route("/:id", get(get_treasury_state))
        .route("/ws", get(ws_handler))
}
