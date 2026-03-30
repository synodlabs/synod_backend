use axum::extract::{Path, State, ws::{WebSocket, WebSocketUpgrade, Message}};
use axum::{routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use bcrypt::{hash, verify, DEFAULT_COST};
use tracing::{info, warn};
use crate::error::{AppError, AppResult};
use crate::AppState;
use crate::auth::AuthUser;

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentSlot {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub agent_pubkey: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub last_connected: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateAgentResponse {
    pub agent: AgentSlot,
    pub api_key: String,
}

#[derive(Debug, Deserialize)]
pub struct HandshakeRequest {
    pub api_key: String,
    pub agent_pubkey: String,
}

pub async fn list_agents(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
) -> AppResult<Json<Vec<AgentSlot>>> {
    let agents = sqlx::query_as!(
        AgentSlot,
        "SELECT agent_id, treasury_id, name, description, agent_pubkey, status, created_at, last_connected 
         FROM agent_slots WHERE treasury_id = $1",
        treasury_id
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(agents))
}

pub async fn create_agent(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<CreateAgentRequest>,
) -> AppResult<(axum::http::StatusCode, Json<CreateAgentResponse>)> {
    // 1. Generate API Key
    let api_key = format!("synod_{}", Uuid::new_v4().to_string().replace("-", ""));
    let api_key_hash = hash(&api_key, DEFAULT_COST).map_err(|e| AppError::Internal(e.into()))?;

    // 2. Insert into DB
    let agent_id = Uuid::new_v4();
    let now = Utc::now();
    
    sqlx::query!(
        "INSERT INTO agent_slots (agent_id, treasury_id, name, description, api_key_hash, created_at) 
         VALUES ($1, $2, $3, $4, $5, $6)",
        agent_id, treasury_id, payload.name, payload.description, api_key_hash, now
    )
    .execute(&state.db)
    .await?;

    let agent = AgentSlot {
        agent_id,
        treasury_id,
        name: payload.name,
        description: payload.description,
        agent_pubkey: None,
        status: "PENDING_CONNECTION".to_string(),
        created_at: now,
        last_connected: None,
    };

    info!(treasury = %treasury_id, agent = %agent_id, "New agent slot created");

    Ok((axum::http::StatusCode::CREATED, Json(CreateAgentResponse { agent, api_key })))
}

pub async fn agent_handshake(
    State(state): State<AppState>,
    Json(payload): Json<HandshakeRequest>,
) -> AppResult<Json<AgentSlot>> {
    // 1. Find agent by some identifier? We need an ID or just search by API key hash? 
    // Usually API key hash is unique.
    // However, bcrypt verify is slow so we can't easily query by it.
    // We should probably pass the agent_id too, OR use a faster hash prefix (not needed for now).
    
    // For now, let's assume the agent provides their agent_id in the handshake or we search all pending.
    // Better: the handshake should probably include agent_id.
    
    // Wait, let's update HandshakeRequest to include agent_id.
    let agent_row = sqlx::query!(
        "SELECT agent_id, treasury_id, api_key_hash, status FROM agent_slots WHERE agent_pubkey IS NULL OR status = 'PENDING_CONNECTION'"
    )
    .fetch_all(&state.db)
    .await?;

    for row in agent_row {
        if verify(&payload.api_key, &row.api_key_hash).unwrap_or(false) {
            // Found it!
            sqlx::query!(
                "UPDATE agent_slots SET agent_pubkey = $1, status = 'ACTIVE', last_connected = $2 WHERE agent_id = $3",
                payload.agent_pubkey, Utc::now(), row.agent_id
            )
            .execute(&state.db)
            .await?;

            let agent = sqlx::query_as!(
                AgentSlot,
                "SELECT agent_id, treasury_id, name, description, agent_pubkey, status, created_at, last_connected 
                 FROM agent_slots WHERE agent_id = $1",
                row.agent_id
            )
            .fetch_one(&state.db)
            .await?;

            info!(agent = %row.agent_id, "Agent handshake completed");
            return Ok(Json(agent));
        }
    }

    Err(AppError::InvalidApiKey)
}

pub async fn suspend_agent(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((treasury_id, agent_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<AgentSlot>> {
    sqlx::query!(
        "UPDATE agent_slots SET status = 'SUSPENDED', suspended_at = $1 WHERE agent_id = $2 AND treasury_id = $3",
        Utc::now(), agent_id, treasury_id
    )
    .execute(&state.db)
    .await?;

    let agent = sqlx::query_as!(
        AgentSlot,
        "SELECT agent_id, treasury_id, name, description, agent_pubkey, status, created_at, last_connected 
         FROM agent_slots WHERE agent_id = $1",
        agent_id
    )
    .fetch_one(&state.db)
    .await?;

    Ok(Json(agent))
}

// ── WebSocket Handler ──

pub async fn agent_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    // In a real app, we'd authenticate the WS connection (e.g. via token in query param)
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state, agent_id))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, agent_id: Uuid) {
    info!(agent = %agent_id, "Agent WebSocket connected");

    // 1. Get Agent's Treasury ID
    let treasury_id = match sqlx::query!("SELECT treasury_id FROM agent_slots WHERE agent_id = $1", agent_id)
        .fetch_one(&state.db)
        .await {
            Ok(row) => row.treasury_id,
            Err(_) => {
                warn!(agent = %agent_id, "Agent not found for WS");
                return;
            }
        };

    let mut rx = state.tx_events.subscribe();
    
    loop {
        tokio::select! {
            // Receive from WebSocket (Client -> Coordinator)
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if text == "ping" {
                            let _ = socket.send(Message::Text("pong".to_string())).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            // Receive from Broadcast (Coordinator -> Agent)
            event = rx.recv() => {
                match event {
                    Ok(crate::TreasuryEvent::PoolBalanceUpdate { treasury_id: tid, pool_key, amount, asset_code }) if tid == treasury_id => {
                        info!(agent = %agent_id, "WS sending PoolBalanceUpdate");
                        let payload = serde_json::json!({
                            "type": "POOL_BALANCE_UPDATE",
                            "pool_key": pool_key,
                            "amount": amount,
                            "asset_code": asset_code
                        });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::ConstitutionUpdate { treasury_id: tid, version }) if tid == treasury_id => {
                        info!(agent = %agent_id, version = version, "WS sending ConstitutionUpdate");
                        let payload = serde_json::json!({
                            "type": "CONSTITUTION_UPDATE",
                            "version": version
                        });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        warn!(agent = %agent_id, "Agent WS lagged behind events");
                    }
                    Err(_) => break,
                    _ => {} // Ignore events for other treasuries
                }
            }
        }
    }
    
    info!(agent = %agent_id, "Agent WebSocket disconnected");
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:treasury_id", get(list_agents).post(create_agent))
        .route("/:treasury_id/:agent_id/suspend", post(suspend_agent))
        .route("/handshake", post(agent_handshake))
        .route("/ws/:agent_id", get(agent_ws))
}
