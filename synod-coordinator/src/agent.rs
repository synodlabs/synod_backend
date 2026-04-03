use axum::extract::{Path, Query, State, ws::{WebSocket, WebSocketUpgrade, Message}};
use axum::{routing::{get, post}, Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use bcrypt::{hash, verify, DEFAULT_COST};
use tracing::{info, warn};
use crate::error::{AppError, AppResult};
use crate::AppState;
use crate::auth::AuthUser;
use crate::constitution::{normalize_constitution_value, rules_for_agent, AgentWalletRule as ConstitutionAgentWalletRule};
use sqlx::Row;

// ── Data Structures ──

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentSlot {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub wallet_address: Option<String>,
    pub agent_pubkey: Option<String>,
    pub status: String,
    pub allocation_pct: f64,
    pub tier_limit_usd: f64,
    pub concurrent_permit_cap: i32,
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

#[derive(Debug, Serialize)]
pub struct WalletAccess {
    pub wallet_address: String,
    pub allocation_pct: f64,
    pub tier_limit_usd: f64,
    pub concurrent_permit_cap: i32,
    pub current_wallet_aum_usd: String,
    pub agent_max_usd: String,
}

#[derive(Debug, Serialize)]
pub struct ConnectResponse {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub slot_status: String,
    pub connection_phase: String,
    pub reason_code: Option<String>,
    pub wallet_access: Vec<WalletAccess>,
    pub websocket_endpoint: String,
    pub coordinator_pubkey: String,
}

#[derive(Debug, Serialize)]
pub struct AgentStatusResponse {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub slot_status: String,
    pub connection_phase: String,
    pub reason_code: Option<String>,
    pub name: String,
    pub agent_pubkey: Option<String>,
    pub wallet_access: Vec<WalletAccess>,
    pub last_connected: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct ConnectStatusResponse {
    pub agent_id: Uuid,
    pub slot_status: String,
    pub connection_phase: String,
    pub reason_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WsAuthQuery {
    pub token: Option<String>,
}

// ── Helpers ──

fn row_to_agent_slot(row: &sqlx::postgres::PgRow) -> AgentSlot {
    AgentSlot {
        agent_id: row.get("agent_id"),
        treasury_id: row.get("treasury_id"),
        name: row.get("name"),
        description: row.get("description"),
        wallet_address: row.get("wallet_address"),
        agent_pubkey: row.get("agent_pubkey"),
        status: row.get("status"),
        allocation_pct: row.get("allocation_pct"),
        tier_limit_usd: row.get("tier_limit_usd"),
        concurrent_permit_cap: row.get("concurrent_permit_cap"),
        created_at: row.get("created_at"),
        last_connected: row.get("last_connected"),
    }
}

async fn load_constitution_rules_for_agent(
    db: &sqlx::PgPool,
    treasury_id: Uuid,
    agent_id: Uuid,
) -> AppResult<Vec<ConstitutionAgentWalletRule>> {
    let content_json: serde_json::Value = sqlx::query_scalar(
        "SELECT content FROM constitution_history WHERE treasury_id = $1 ORDER BY version DESC LIMIT 1"
    )
    .bind(treasury_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| AppError::NotFound("Constitution not found".into()))?;

    let content = normalize_constitution_value(content_json)?;
    Ok(rules_for_agent(&content, agent_id))
}

/// Build wallet access config for an agent, computing headroom from treasury AUM.
async fn build_wallet_access(
    db: &sqlx::PgPool,
    treasury_id: Uuid,
    rules: &[ConstitutionAgentWalletRule],
) -> AppResult<Vec<WalletAccess>> {
    let row = sqlx::query(
        "SELECT current_aum_usd::float8 FROM treasuries WHERE treasury_id = $1"
    )
    .bind(treasury_id)
    .fetch_optional(db).await?;

    let aum: f64 = row.map(|r| r.get::<f64, _>("current_aum_usd")).unwrap_or(0.0);

    Ok(rules.iter().map(|rule| WalletAccess {
        wallet_address: rule.wallet_address.clone(),
        allocation_pct: rule.allocation_pct,
        tier_limit_usd: rule.tier_limit_usd,
        concurrent_permit_cap: rule.concurrent_permit_cap,
        current_wallet_aum_usd: format!("{:.2}", aum),
        agent_max_usd: format!("{:.2}", aum * (rule.allocation_pct / 100.0)),
    }).collect())
}

async fn compute_connection_status(
    state: &AppState,
    agent: &AgentSlot,
) -> AppResult<(String, Option<String>, Vec<ConstitutionAgentWalletRule>)> {
    if agent.status == "REVOKED" {
        return Ok(("FAILED".to_string(), Some("AGENT_REVOKED".to_string()), vec![]));
    }

    if agent.status == "SUSPENDED" {
        return Ok(("FAILED".to_string(), Some("AGENT_SUSPENDED".to_string()), vec![]));
    }

    let rules = load_constitution_rules_for_agent(&state.db, agent.treasury_id, agent.agent_id).await?;
    if rules.is_empty() {
        return Ok(("PENDING".to_string(), Some("PENDING_CONFIGURATION".to_string()), rules));
    }

    let Some(agent_pubkey) = agent.agent_pubkey.clone() else {
        return Ok(("PENDING".to_string(), Some("PENDING_SIGNER".to_string()), rules));
    };

    for rule in &rules {
        let is_signer = check_signer_on_chain(
            &state.config.stellar.horizon_url,
            &rule.wallet_address,
            &agent_pubkey,
        )
        .await
        .unwrap_or(false);

        if !is_signer {
            return Ok(("PENDING".to_string(), Some("PENDING_SIGNER".to_string()), rules));
        }
    }

    Ok(("COMPLETE".to_string(), None, rules))
}

/// Check via Horizon REST whether a pubkey is already a signer on a wallet.
async fn check_signer_on_chain(
    horizon_url: &str,
    wallet_address: &str,
    agent_pubkey: &str,
) -> Result<bool, anyhow::Error> {
    let url = format!("{}/accounts/{}", horizon_url, wallet_address);
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await?;

    if !resp.status().is_success() {
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        return Err(anyhow::anyhow!("Horizon returned {} for account {}", resp.status(), wallet_address));
    }

    let body: serde_json::Value = resp.json().await?;
    let signers = body["signers"].as_array();

    if let Some(signers) = signers {
        for signer in signers {
            if signer["key"].as_str() == Some(agent_pubkey) {
                let weight = signer["weight"].as_i64().unwrap_or(0);
                if weight >= 1 {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

// ── Dashboard Endpoints (Auth Required) ──

pub async fn list_agents(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
) -> AppResult<Json<Vec<AgentSlot>>> {
    let agents = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status, 
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected 
         FROM agent_slots WHERE treasury_id = $1 AND status != 'REVOKED' ORDER BY created_at ASC"#
    )
    .bind(treasury_id)
    .fetch_all(&state.db)
    .await?
    .iter()
    .map(row_to_agent_slot)
    .collect();

    Ok(Json(agents))
}

pub async fn create_agent(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<CreateAgentRequest>,
) -> AppResult<(axum::http::StatusCode, Json<CreateAgentResponse>)> {
    let trimmed_name = payload.name.trim();
    if trimmed_name.is_empty() || trimmed_name.len() > 64 {
        return Err(AppError::InvalidInput("Agent name must be between 1 and 64 characters".into()));
    }
    if payload.description.as_ref().is_some_and(|value| value.len() > 255) {
        return Err(AppError::InvalidInput("Description must be 255 characters or fewer".into()));
    }

    let api_key = format!("synod_agent_{}", Uuid::new_v4().to_string().replace("-", ""));
    let api_key_hash = hash(&api_key, DEFAULT_COST).map_err(|e| AppError::Internal(e.into()))?;

    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    let fast_token_hash = hex::encode(hasher.finalize());

    let agent_id = Uuid::new_v4();
    let now = Utc::now();
    
    sqlx::query(
        r#"INSERT INTO agent_slots (agent_id, treasury_id, name, description, api_key_hash, created_at, fast_token_hash)
         VALUES ($1, $2, $3, $4, $5, $6, $7)"#
    )
    .bind(agent_id)
    .bind(treasury_id)
    .bind(trimmed_name)
    .bind(&payload.description)
    .bind(api_key_hash)
    .bind(now)
    .bind(fast_token_hash)
    .execute(&state.db)
    .await?;

    let agent = AgentSlot {
        agent_id,
        treasury_id,
        name: trimmed_name.to_string(),
        description: payload.description,
        wallet_address: None,
        agent_pubkey: None,
        status: "PENDING_CONNECTION".to_string(),
        allocation_pct: 0.0,
        tier_limit_usd: 0.0,
        concurrent_permit_cap: 0,
        created_at: now,
        last_connected: None,
    };

    info!(treasury = %treasury_id, agent = %agent_id, "New agent slot created");

    // Emit event
    let _ = state.tx_events.send(crate::TreasuryEvent::AgentStatusChanged {
        treasury_id,
        agent_id,
        new_status: "PENDING_CONNECTION".to_string(),
    });

    Ok((axum::http::StatusCode::CREATED, Json(CreateAgentResponse { agent, api_key })))
}

pub async fn agent_connect_status(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<ConnectStatusResponse>> {
    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
           FROM agent_slots WHERE agent_id = $1"#
    )
        .bind(agent_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::AgentNotFound)?;
    let agent = row_to_agent_slot(&row);
    let (connection_phase, reason_code, rules) = compute_connection_status(&state, &agent).await?;

    if connection_phase == "COMPLETE" && agent.status != "ACTIVE" {
        sqlx::query("UPDATE agent_slots SET status = 'ACTIVE', last_connected = $1 WHERE agent_id = $2")
            .bind(Utc::now())
            .bind(agent.agent_id)
            .execute(&state.db)
            .await?;
    }

    Ok(Json(ConnectStatusResponse {
        agent_id: agent.agent_id,
        slot_status: if connection_phase == "COMPLETE" { "ACTIVE".to_string() } else { agent.status.clone() },
        connection_phase,
        reason_code: reason_code.or_else(|| if rules.is_empty() { Some("PENDING_CONFIGURATION".to_string()) } else { None }),
    }))
}

// ── Agent Handshake (No Auth — uses API key) ──

pub async fn agent_handshake(
    State(state): State<AppState>,
    Json(payload): Json<HandshakeRequest>,
) -> AppResult<Json<ConnectResponse>> {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(payload.api_key.as_bytes());
    let token_hash = hex::encode(hasher.finalize());
    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at,
           last_connected, api_key_hash
         FROM agent_slots WHERE fast_token_hash = $1"#
    )
    .bind(&token_hash)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::InvalidApiKey)?;
    let mut agent = row_to_agent_slot(&row);
    let hash: String = row.get("api_key_hash");
    if !verify(&payload.api_key, &hash).unwrap_or(false) {
        return Err(AppError::InvalidApiKey);
    }
    match agent.status.as_str() {
        "REVOKED" => return Err(AppError::AgentRevoked),
        "SUSPENDED" => return Err(AppError::AgentSuspended),
        "PENDING_CONNECTION" | "INACTIVE" | "ACTIVE" => {}
        other => {
            warn!(agent = %agent.agent_id, status = %other, "Unknown agent status during handshake");
        }
    }
    if let Some(ref existing_pubkey) = agent.agent_pubkey {
        if existing_pubkey != &payload.agent_pubkey {
            return Err(AppError::PubkeyConflict);
        }
    } else {
        sqlx::query("UPDATE agent_slots SET agent_pubkey = $1 WHERE agent_id = $2")
            .bind(&payload.agent_pubkey)
            .bind(agent.agent_id)
            .execute(&state.db)
            .await?;
        agent.agent_pubkey = Some(payload.agent_pubkey.clone());
        info!(agent = %agent.agent_id, pubkey = %payload.agent_pubkey, "Agent pubkey registered");
    }
    let (connection_phase, reason_code, rules) = compute_connection_status(&state, &agent).await?;
    let wallet_access = build_wallet_access(&state.db, agent.treasury_id, &rules).await?;
    if connection_phase != "COMPLETE" {
        let _ = state.tx_events.send(crate::TreasuryEvent::AgentConnected {
            treasury_id: agent.treasury_id,
            agent_id: agent.agent_id,
        });
        return Ok(Json(ConnectResponse {
            agent_id: agent.agent_id,
            treasury_id: agent.treasury_id,
            slot_status: agent.status.clone(),
            connection_phase,
            reason_code,
            wallet_access,
            websocket_endpoint: format!("/v1/agents/ws/{}", agent.agent_id),
            coordinator_pubkey: state.config.stellar.coordinator_pubkey.clone(),
        }));
    }
    sqlx::query(
        "UPDATE agent_slots SET status = 'ACTIVE', last_connected = $1 WHERE agent_id = $2"
    )
    .bind(Utc::now())
    .bind(agent.agent_id)
    .execute(&state.db)
    .await?;
    let _ = state.tx_events.send(crate::TreasuryEvent::AgentStatusChanged {
        treasury_id: agent.treasury_id,
        agent_id: agent.agent_id,
        new_status: "ACTIVE".to_string(),
    });
    let _ = state.tx_events.send(crate::TreasuryEvent::AgentActivated {
        treasury_id: agent.treasury_id,
        agent_id: agent.agent_id,
    });
    let next_seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM events WHERE treasury_id = $1"
    )
    .bind(agent.treasury_id)
    .fetch_one(&state.db)
    .await?;
    sqlx::query(
        "INSERT INTO events (treasury_id, event_type, sequence, payload) VALUES ($1, $2, $3, $4)"
    )
    .bind(agent.treasury_id)
    .bind("AGENT_CONNECTED")
    .bind(next_seq)
    .bind(serde_json::json!({
        "agent_id": agent.agent_id,
        "agent_pubkey": payload.agent_pubkey,
    }))
    .execute(&state.db)
    .await?;
    info!(agent = %agent.agent_id, "Agent handshake completed - ACTIVE");
    Ok(Json(ConnectResponse {
        agent_id: agent.agent_id,
        treasury_id: agent.treasury_id,
        slot_status: "ACTIVE".to_string(),
        connection_phase: "COMPLETE".to_string(),
        reason_code: None,
        wallet_access,
        websocket_endpoint: format!("/v1/agents/ws/{}", agent.agent_id),
        coordinator_pubkey: state.config.stellar.coordinator_pubkey.clone(),
    }))
}
// Lifecycle Endpoints

pub async fn agent_status(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<AgentStatusResponse>> {
    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
         FROM agent_slots WHERE agent_id = $1"#
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::AgentNotFound)?;
    let agent = row_to_agent_slot(&row);
    let (connection_phase, reason_code, rules) = compute_connection_status(&state, &agent).await?;
    let wallet_access = build_wallet_access(&state.db, agent.treasury_id, &rules).await?;
    if connection_phase == "COMPLETE" && agent.status != "ACTIVE" {
        sqlx::query("UPDATE agent_slots SET status = 'ACTIVE', last_connected = $1 WHERE agent_id = $2")
            .bind(Utc::now())
            .bind(agent.agent_id)
            .execute(&state.db)
            .await?;
    }
    Ok(Json(AgentStatusResponse {
        agent_id: agent.agent_id,
        treasury_id: agent.treasury_id,
        slot_status: if connection_phase == "COMPLETE" { "ACTIVE".to_string() } else { agent.status.clone() },
        connection_phase,
        reason_code,
        name: agent.name,
        agent_pubkey: agent.agent_pubkey,
        wallet_access,
        last_connected: agent.last_connected,
    }))
}

pub async fn agent_heartbeat(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<axum::http::StatusCode> {
    let rows = sqlx::query(
        "UPDATE agent_slots SET last_connected = $1, status = CASE WHEN status = 'INACTIVE' THEN 'ACTIVE' ELSE status END WHERE agent_id = $2 AND status IN ('ACTIVE', 'INACTIVE') RETURNING status"
    )
    .bind(Utc::now())
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await?;

    if rows.is_none() {
        return Err(AppError::AgentNotFound);
    }

    Ok(axum::http::StatusCode::OK)
}

pub async fn suspend_agent(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((treasury_id, agent_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<AgentSlot>> {
    sqlx::query(
        "UPDATE agent_slots SET status = 'SUSPENDED', suspended_at = $1 WHERE agent_id = $2 AND treasury_id = $3"
    )
    .bind(Utc::now())
    .bind(agent_id)
    .bind(treasury_id)
    .execute(&state.db)
    .await?;

    // Emit suspension event
    let _ = state.tx_events.send(crate::TreasuryEvent::AgentSuspended {
        treasury_id,
        agent_id,
    });

    // Log to events table
    let next_seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM events WHERE treasury_id = $1"
    )
    .bind(treasury_id)
    .fetch_one(&state.db)
    .await?;

    sqlx::query(
        "INSERT INTO events (treasury_id, event_type, sequence, payload) VALUES ($1, $2, $3, $4)"
    )
    .bind(treasury_id)
    .bind("AGENT_SUSPENDED")
    .bind(next_seq)
    .bind(serde_json::json!({ "agent_id": agent_id }))
    .execute(&state.db)
    .await?;

    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status, 
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected 
         FROM agent_slots WHERE agent_id = $1"#
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(row_to_agent_slot(&row)))
}

pub async fn revoke_agent(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((treasury_id, agent_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<AgentSlot>> {
    sqlx::query(
        "UPDATE agent_slots SET status = 'REVOKED' WHERE agent_id = $1 AND treasury_id = $2"
    )
    .bind(agent_id)
    .bind(treasury_id)
    .execute(&state.db)
    .await?;

    let next_seq: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM events WHERE treasury_id = $1"
    )
    .bind(treasury_id)
    .fetch_one(&state.db)
    .await?;

    sqlx::query(
        "INSERT INTO events (treasury_id, event_type, sequence, payload) VALUES ($1, $2, $3, $4)"
    )
    .bind(treasury_id)
    .bind("AGENT_REVOKED")
    .bind(next_seq)
    .bind(serde_json::json!({ "agent_id": agent_id }))
    .execute(&state.db)
    .await?;

    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status, 
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected 
         FROM agent_slots WHERE agent_id = $1"#
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(row_to_agent_slot(&row)))
}

// ── WebSocket Handler ──

pub async fn agent_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Query(_query): Query<WsAuthQuery>,
) -> impl axum::response::IntoResponse {
    // TODO: validate query.token against agent's API key or session token
    ws.on_upgrade(move |socket| handle_socket(socket, state, agent_id))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, agent_id: Uuid) {
    info!(agent = %agent_id, "Agent WebSocket connected");

    // Get Agent's Treasury ID and active rule view
    let agent_info = match sqlx::query(
        "SELECT treasury_id FROM agent_slots WHERE agent_id = $1"
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await {
        Ok(row) => row,
        Err(_) => {
            warn!(agent = %agent_id, "Agent not found for WS");
            return;
        }
    };

    let treasury_id: Uuid = agent_info.get("treasury_id");
    let wallet_allocations: std::collections::HashMap<String, f64> = load_constitution_rules_for_agent(&state.db, treasury_id, agent_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|rule| (rule.wallet_address, rule.allocation_pct))
        .collect();

    let mut rx = state.tx_events.subscribe();
    
    loop {
        tokio::select! {
            // Client → Coordinator
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
            // Coordinator → Agent (broadcast)
            event = rx.recv() => {
                match event {
                    Ok(crate::TreasuryEvent::WalletBalanceUpdate { treasury_id: tid, wallet_address, amount, asset_code }) if tid == treasury_id => {
                        let Some(allocation_pct) = wallet_allocations.get(&wallet_address) else {
                            continue;
                        };
                        let agent_new_max = amount * (allocation_pct / 100.0);
                        let payload = serde_json::json!({
                            "type": "WALLET_AUM_UPDATE",
                            "wallet_address": wallet_address,
                            "new_aum_usd": format!("{:.2}", amount),
                            "agent_new_max_usd": format!("{:.2}", agent_new_max),
                            "asset_code": asset_code
                        });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::ConstitutionUpdate { treasury_id: tid, version }) if tid == treasury_id => {
                        let payload = serde_json::json!({
                            "type": "CONSTITUTION_UPDATED",
                            "version": version
                        });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::PermitIssued { treasury_id: tid, agent_id: aid, permit_id, wallet_address, approved_amount }) if tid == treasury_id => {
                        let payload = serde_json::json!({
                            "type": "PERMIT_ISSUED",
                            "agent_id": aid,
                            "permit_id": permit_id,
                            "wallet_address": wallet_address,
                            "approved_amount": approved_amount
                        });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::PermitConsumed { treasury_id: tid, permit_id, wallet_address }) if tid == treasury_id => {
                        let payload = serde_json::json!({
                            "type": "PERMIT_CONSUMED",
                            "permit_id": permit_id,
                            "wallet_address": wallet_address
                        });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::PermitExpired { treasury_id: tid, permit_id, wallet_address }) if tid == treasury_id => {
                        let payload = serde_json::json!({
                            "type": "PERMIT_EXPIRED",
                            "permit_id": permit_id,
                            "wallet_address": wallet_address
                        });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::TreasuryHalted { treasury_id: tid }) if tid == treasury_id => {
                        let payload = serde_json::json!({ "type": "TREASURY_HALTED" });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::TreasuryResumed { treasury_id: tid }) if tid == treasury_id => {
                        let payload = serde_json::json!({ "type": "TREASURY_RESUMED" });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::AgentSuspended { treasury_id: tid, agent_id: aid }) if tid == treasury_id && aid == agent_id => {
                        let payload = serde_json::json!({ "type": "AGENT_SUSPENDED" });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::AgentStatusChanged { treasury_id: tid, agent_id: aid, new_status }) if tid == treasury_id && aid == agent_id => {
                        let payload = serde_json::json!({
                            "type": "AGENT_STATUS_CHANGED",
                            "new_status": new_status
                        });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        warn!(agent = %agent_id, "Agent WS lagged behind events");
                    }
                    Err(_) => break,
                    _ => {} // Ignore events for other treasuries/agents
                }
            }
        }
    }
    
    info!(agent = %agent_id, "Agent WebSocket disconnected");
}

// ── Router ──

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:treasury_id", get(list_agents).post(create_agent))
        .route("/:treasury_id/:agent_id/suspend", post(suspend_agent))
        .route("/:treasury_id/:agent_id/revoke", post(revoke_agent))
        .route("/connect", post(agent_handshake))
        .route("/connect/status/:agent_id", get(agent_connect_status))
        .route("/:agent_id/status", get(agent_status))
        .route("/:agent_id/heartbeat", post(agent_heartbeat))
        .route("/ws/:agent_id", get(agent_ws))
}


