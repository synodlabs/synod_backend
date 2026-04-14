use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::{info, warn};
use uuid::Uuid;

use crate::auth::{load_agent_session, store_agent_session, AgentAuth, AgentSession, AuthUser};
use crate::constitution::{
    normalize_constitution_value, rules_for_agent,
    AgentWalletRule as ConstitutionAgentWalletRule,
};
use crate::error::{AppError, AppResult};
use crate::stellar;
use crate::AppState;

const ENROLL_CHALLENGE_TTL_SECS: u64 = 600;
const CONNECT_CHALLENGE_TTL_SECS: u64 = 300;
const AGENT_SESSION_TTL_SECS: u64 = 3600;

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

#[derive(Debug, Deserialize)]
pub struct CreateAgentBodyRequest {
    pub treasury_id: Uuid,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateAgentResponse {
    pub agent: AgentSlot,
}

#[derive(Debug, Deserialize)]
pub struct EnrollChallengeRequest {
    pub wallet_address: String,
    pub agent_pubkey: String,
}

#[derive(Debug, Serialize)]
pub struct EnrollChallengeResponse {
    pub challenge: String,
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub wallet_address: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct EnrollPubkeyRequest {
    pub wallet_address: String,
    pub agent_pubkey: String,
    pub challenge: String,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
pub struct ConnectChallengeRequest {
    pub agent_pubkey: String,
}

#[derive(Debug, Serialize)]
pub struct ConnectChallengeResponse {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub challenge: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct ConnectCompleteRequest {
    pub agent_pubkey: String,
    pub challenge: String,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshTicketRequest {
    pub websocket_only: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct RefreshTicketResponse {
    pub session_token: String,
    pub websocket_token: String,
    pub expires_at: DateTime<Utc>,
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
    pub websocket_token: String,
    pub session_token: String,
    pub expires_at: DateTime<Utc>,
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

#[derive(Debug, Serialize, Deserialize)]
struct EnrollmentChallenge {
    agent_id: Uuid,
    treasury_id: Uuid,
    wallet_address: String,
    agent_pubkey: String,
    expires_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConnectChallenge {
    agent_id: Uuid,
    treasury_id: Uuid,
    agent_pubkey: String,
    expires_at: i64,
}

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

fn enrich_slot_with_status(slot: &mut AgentSlot, connection_phase: &str, reason_code: Option<&str>) {
    if matches!(slot.status.as_str(), "SUSPENDED" | "REVOKED" | "INACTIVE") {
        return;
    }

    if connection_phase == "COMPLETE" {
        slot.status = "ACTIVE".to_string();
        return;
    }

    if let Some(reason_code) = reason_code {
        slot.status = reason_code.to_string();
    }
}

fn enrollment_challenge_key(agent_id: Uuid) -> String {
    format!("agent:enroll:challenge:{}", agent_id)
}

fn connect_challenge_key(agent_pubkey: &str) -> String {
    format!("agent:connect:challenge:{}", agent_pubkey)
}

fn session_expiry_datetime() -> DateTime<Utc> {
    Utc::now() + chrono::Duration::seconds(AGENT_SESSION_TTL_SECS as i64)
}

fn pending_status_from_reason(reason_code: Option<&str>) -> String {
    match reason_code {
        Some("PENDING_SIGNER") => "PENDING_SIGNER".to_string(),
        Some("PENDING_CONFIGURATION") => "PENDING_CONFIGURATION".to_string(),
        Some("PENDING_PUBKEY") => "PENDING_PUBKEY".to_string(),
        Some("AGENT_REVOKED") => "REVOKED".to_string(),
        Some("AGENT_SUSPENDED") => "SUSPENDED".to_string(),
        _ => "INACTIVE".to_string(),
    }
}

async fn load_constitution_rules_for_agent(
    db: &sqlx::PgPool,
    treasury_id: Uuid,
    agent_id: Uuid,
) -> AppResult<Vec<ConstitutionAgentWalletRule>> {
    let content_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT content FROM constitution_history WHERE treasury_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(treasury_id)
    .fetch_optional(db)
    .await?;

    let Some(content_json) = content_json else {
        return Ok(vec![]);
    };

    let content = normalize_constitution_value(content_json)?;
    Ok(rules_for_agent(&content, agent_id))
}

async fn build_wallet_access(
    db: &sqlx::PgPool,
    treasury_id: Uuid,
    rules: &[ConstitutionAgentWalletRule],
) -> AppResult<Vec<WalletAccess>> {
    let row = sqlx::query("SELECT current_aum_usd::float8 FROM treasuries WHERE treasury_id = $1")
        .bind(treasury_id)
        .fetch_optional(db)
        .await?;

    let aum: f64 = row
        .map(|record| record.get::<f64, _>("current_aum_usd"))
        .unwrap_or(0.0);

    Ok(rules
        .iter()
        .map(|rule| WalletAccess {
            wallet_address: rule.wallet_address.clone(),
            allocation_pct: rule.allocation_pct,
            tier_limit_usd: rule.tier_limit_usd,
            concurrent_permit_cap: rule.concurrent_permit_cap,
            current_wallet_aum_usd: format!("{:.2}", aum),
            agent_max_usd: format!("{:.2}", aum * (rule.allocation_pct / 100.0)),
        })
        .collect())
}

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
        return Err(anyhow::anyhow!(
            "Horizon returned {} for account {}",
            resp.status(),
            wallet_address
        ));
    }

    let body: serde_json::Value = resp.json().await?;
    if let Some(signers) = body["signers"].as_array() {
        for signer in signers {
            if signer["key"].as_str() == Some(agent_pubkey) && signer["weight"].as_i64().unwrap_or(0) >= 1
            {
                return Ok(true);
            }
        }
    }

    Ok(false)
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

    let Some(agent_pubkey) = agent.agent_pubkey.clone() else {
        return Ok(("PENDING".to_string(), Some("PENDING_PUBKEY".to_string()), vec![]));
    };

    let rules = load_constitution_rules_for_agent(&state.db, agent.treasury_id, agent.agent_id).await?;
    if rules.is_empty() {
        return Ok((
            "PENDING".to_string(),
            Some("PENDING_CONFIGURATION".to_string()),
            rules,
        ));
    }

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

async fn create_agent_slot(
    state: &AppState,
    treasury_id: Uuid,
    payload: CreateAgentRequest,
) -> AppResult<(axum::http::StatusCode, Json<CreateAgentResponse>)> {
    let trimmed_name = payload.name.trim();
    if trimmed_name.is_empty() || trimmed_name.len() > 64 {
        return Err(AppError::InvalidInput(
            "Agent name must be between 1 and 64 characters".into(),
        ));
    }

    if payload.description.as_ref().is_some_and(|value| value.len() > 255) {
        return Err(AppError::InvalidInput(
            "Description must be 255 characters or fewer".into(),
        ));
    }

    let agent_id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        r#"INSERT INTO agent_slots (
            agent_id,
            treasury_id,
            name,
            description,
            api_key_hash,
            created_at,
            fast_token_hash,
            status
        ) VALUES ($1, $2, $3, $4, NULL, $5, NULL, 'PENDING_PUBKEY')"#,
    )
    .bind(agent_id)
    .bind(treasury_id)
    .bind(trimmed_name)
    .bind(&payload.description)
    .bind(now)
    .execute(&state.db)
    .await?;

    let agent = AgentSlot {
        agent_id,
        treasury_id,
        name: trimmed_name.to_string(),
        description: payload.description,
        wallet_address: None,
        agent_pubkey: None,
        status: "PENDING_PUBKEY".to_string(),
        allocation_pct: 0.0,
        tier_limit_usd: 0.0,
        concurrent_permit_cap: 0,
        created_at: now,
        last_connected: None,
    };

    let _ = state.tx_events.send(crate::TreasuryEvent::AgentStatusChanged {
        treasury_id,
        agent_id,
        new_status: "PENDING_PUBKEY".to_string(),
    });

    Ok((axum::http::StatusCode::CREATED, Json(CreateAgentResponse { agent })))
}

async fn issue_agent_session(
    state: &AppState,
    agent_id: Uuid,
    treasury_id: Uuid,
    agent_pubkey: &str,
) -> AppResult<RefreshTicketResponse> {
    let session_token = format!(
        "{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );
    let expires_at = session_expiry_datetime();
    let session = AgentSession {
        agent_id,
        treasury_id,
        agent_pubkey: agent_pubkey.to_string(),
        issued_at: Utc::now().timestamp(),
        expires_at: expires_at.timestamp(),
    };

    store_agent_session(state, &session_token, &session, AGENT_SESSION_TTL_SECS).await?;

    Ok(RefreshTicketResponse {
        session_token: session_token.clone(),
        websocket_token: session_token,
        expires_at,
    })
}

pub async fn list_agents(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
) -> AppResult<Json<Vec<AgentSlot>>> {
    let rows = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
         FROM agent_slots WHERE treasury_id = $1 AND status != 'REVOKED' ORDER BY created_at ASC"#,
    )
    .bind(treasury_id)
    .fetch_all(&state.db)
    .await?;

    let mut agents = Vec::with_capacity(rows.len());
    for row in rows {
        let mut slot = row_to_agent_slot(&row);
        let (connection_phase, reason_code, _) = compute_connection_status(&state, &slot).await?;
        enrich_slot_with_status(&mut slot, &connection_phase, reason_code.as_deref());
        agents.push(slot);
    }

    Ok(Json(agents))
}

pub async fn create_agent(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<CreateAgentRequest>,
) -> AppResult<(axum::http::StatusCode, Json<CreateAgentResponse>)> {
    create_agent_slot(&state, treasury_id, payload).await
}

pub async fn create_agent_from_body(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(payload): Json<CreateAgentBodyRequest>,
) -> AppResult<(axum::http::StatusCode, Json<CreateAgentResponse>)> {
    create_agent_slot(
        &state,
        payload.treasury_id,
        CreateAgentRequest {
            name: payload.name,
            description: payload.description,
        },
    )
    .await
}

pub async fn start_pubkey_enrollment(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<EnrollChallengeRequest>,
) -> AppResult<Json<EnrollChallengeResponse>> {
    let row = sqlx::query(
        "SELECT treasury_id, status, agent_pubkey FROM agent_slots WHERE agent_id = $1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::AgentNotFound)?;

    let treasury_id: Uuid = row.get("treasury_id");
    let status: String = row.get("status");
    let existing_pubkey: Option<String> = row.get("agent_pubkey");

    match status.as_str() {
        "SUSPENDED" => return Err(AppError::AgentSuspended),
        "REVOKED" => return Err(AppError::AgentRevoked),
        _ => {}
    }

    if let Some(existing_pubkey) = existing_pubkey {
        if existing_pubkey != payload.agent_pubkey {
            return Err(AppError::PubkeyConflict);
        }
    }

    let wallet_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM treasury_wallets
            WHERE treasury_id = $1 AND wallet_address = $2 AND status = 'ACTIVE'
        )",
    )
    .bind(treasury_id)
    .bind(&payload.wallet_address)
    .fetch_one(&state.db)
    .await?;

    if !wallet_exists {
        return Err(AppError::WalletNotFound);
    }

    let challenge = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + chrono::Duration::seconds(ENROLL_CHALLENGE_TTL_SECS as i64);
    let record = EnrollmentChallenge {
        agent_id,
        treasury_id,
        wallet_address: payload.wallet_address.clone(),
        agent_pubkey: payload.agent_pubkey.clone(),
        expires_at: expires_at.timestamp(),
    };

    let mut redis_conn = state.redis.clone();
    let value = serde_json::to_string(&record)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Challenge encode failed: {}", e)))?;
    let _: () = redis_conn
        .set_ex(
            enrollment_challenge_key(agent_id),
            format!("{}|{}", challenge, value),
            ENROLL_CHALLENGE_TTL_SECS,
        )
        .await
        .map_err(AppError::Redis)?;

    Ok(Json(EnrollChallengeResponse {
        challenge,
        agent_id,
        treasury_id,
        wallet_address: payload.wallet_address,
        expires_at,
    }))
}

pub async fn enroll_pubkey(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(agent_id): Path<Uuid>,
    Json(payload): Json<EnrollPubkeyRequest>,
) -> AppResult<Json<AgentSlot>> {
    let mut redis_conn = state.redis.clone();
    let stored: Option<String> = redis_conn
        .get(enrollment_challenge_key(agent_id))
        .await
        .map_err(AppError::Redis)?;
    let stored = stored.ok_or(AppError::ChallengeExpired)?;
    let (challenge, challenge_json) = stored
        .split_once('|')
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Malformed enrollment challenge")))?;

    if challenge != payload.challenge {
        return Err(AppError::ChallengeExpired);
    }

    let record: EnrollmentChallenge = serde_json::from_str(challenge_json)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Challenge decode failed: {}", e)))?;

    if record.expires_at <= Utc::now().timestamp()
        || record.wallet_address != payload.wallet_address
        || record.agent_pubkey != payload.agent_pubkey
    {
        return Err(AppError::ChallengeExpired);
    }

    let message = format!(
        "synod-enroll:{}:{}:{}:{}",
        record.agent_id, record.wallet_address, record.agent_pubkey, payload.challenge
    );

    stellar::verify_stellar_signature(
        &payload.wallet_address,
        message.as_bytes(),
        &payload.signature,
        &state.config.stellar.network_passphrase,
    )?;

    let _: redis::RedisResult<()> = redis_conn.del(enrollment_challenge_key(agent_id)).await;

    sqlx::query(
        "UPDATE agent_slots
         SET agent_pubkey = $1,
             status = $2
         WHERE agent_id = $3",
    )
    .bind(&payload.agent_pubkey)
    .bind("PENDING_SIGNER")
    .bind(agent_id)
    .execute(&state.db)
    .await?;

    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
         FROM agent_slots WHERE agent_id = $1"#,
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await?;
    let mut slot = row_to_agent_slot(&row);
    let (connection_phase, reason_code, _) = compute_connection_status(&state, &slot).await?;
    let new_status = pending_status_from_reason(reason_code.as_deref());
    sqlx::query("UPDATE agent_slots SET status = $1 WHERE agent_id = $2")
        .bind(&new_status)
        .bind(agent_id)
        .execute(&state.db)
        .await?;
    slot.status = new_status.clone();
    enrich_slot_with_status(&mut slot, &connection_phase, reason_code.as_deref());

    let _ = state.tx_events.send(crate::TreasuryEvent::AgentStatusChanged {
        treasury_id: slot.treasury_id,
        agent_id,
        new_status,
    });

    Ok(Json(slot))
}

pub async fn agent_connect_status(
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<ConnectStatusResponse>> {
    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
           FROM agent_slots WHERE agent_id = $1"#,
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::AgentNotFound)?;

    let slot = row_to_agent_slot(&row);
    let (connection_phase, reason_code, _) = compute_connection_status(&state, &slot).await?;
    let slot_status = if connection_phase == "COMPLETE" {
        if slot.status == "INACTIVE" {
            "INACTIVE".to_string()
        } else {
            "ACTIVE".to_string()
        }
    } else {
        pending_status_from_reason(reason_code.as_deref())
    };

    Ok(Json(ConnectStatusResponse {
        agent_id: slot.agent_id,
        slot_status,
        connection_phase,
        reason_code,
    }))
}

pub async fn connect_challenge(
    State(state): State<AppState>,
    Json(payload): Json<ConnectChallengeRequest>,
) -> AppResult<Json<ConnectChallengeResponse>> {
    let row = sqlx::query(
        "SELECT agent_id, treasury_id, status FROM agent_slots WHERE agent_pubkey = $1",
    )
    .bind(&payload.agent_pubkey)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::AgentNotFound)?;

    let agent_id: Uuid = row.get("agent_id");
    let treasury_id: Uuid = row.get("treasury_id");
    let status: String = row.get("status");

    match status.as_str() {
        "SUSPENDED" => return Err(AppError::AgentSuspended),
        "REVOKED" => return Err(AppError::AgentRevoked),
        _ => {}
    }

    let challenge = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + chrono::Duration::seconds(CONNECT_CHALLENGE_TTL_SECS as i64);
    let record = ConnectChallenge {
        agent_id,
        treasury_id,
        agent_pubkey: payload.agent_pubkey.clone(),
        expires_at: expires_at.timestamp(),
    };

    let mut redis_conn = state.redis.clone();
    let value = serde_json::to_string(&record)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Challenge encode failed: {}", e)))?;
    let _: () = redis_conn
        .set_ex(
            connect_challenge_key(&payload.agent_pubkey),
            format!("{}|{}", challenge, value),
            CONNECT_CHALLENGE_TTL_SECS,
        )
        .await
        .map_err(AppError::Redis)?;

    Ok(Json(ConnectChallengeResponse {
        agent_id,
        treasury_id,
        challenge,
        expires_at,
    }))
}

pub async fn connect_complete(
    State(state): State<AppState>,
    Json(payload): Json<ConnectCompleteRequest>,
) -> AppResult<Json<ConnectResponse>> {
    let mut redis_conn = state.redis.clone();
    let stored: Option<String> = redis_conn
        .get(connect_challenge_key(&payload.agent_pubkey))
        .await
        .map_err(AppError::Redis)?;
    let stored = stored.ok_or(AppError::ChallengeExpired)?;
    let (challenge, challenge_json) = stored
        .split_once('|')
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Malformed connect challenge")))?;

    if challenge != payload.challenge {
        return Err(AppError::ChallengeExpired);
    }

    let record: ConnectChallenge = serde_json::from_str(challenge_json)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Challenge decode failed: {}", e)))?;

    if record.expires_at <= Utc::now().timestamp() {
        return Err(AppError::ChallengeExpired);
    }

    let message = format!(
        "synod-connect:{}:{}:{}:{}",
        record.agent_id, record.treasury_id, record.agent_pubkey, payload.challenge
    );

    stellar::verify_stellar_signature(
        &payload.agent_pubkey,
        message.as_bytes(),
        &payload.signature,
        &state.config.stellar.network_passphrase,
    )?;

    let _: redis::RedisResult<()> = redis_conn.del(connect_challenge_key(&payload.agent_pubkey)).await;

    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
         FROM agent_slots WHERE agent_id = $1"#,
    )
    .bind(record.agent_id)
    .fetch_one(&state.db)
    .await?;
    let mut agent = row_to_agent_slot(&row);
    let (connection_phase, reason_code, rules) = compute_connection_status(&state, &agent).await?;
    let wallet_access = build_wallet_access(&state.db, agent.treasury_id, &rules).await?;
    let ticket = issue_agent_session(&state, agent.agent_id, agent.treasury_id, &payload.agent_pubkey).await?;
    let next_status = if connection_phase == "COMPLETE" {
        "ACTIVE".to_string()
    } else {
        pending_status_from_reason(reason_code.as_deref())
    };

    sqlx::query("UPDATE agent_slots SET status = $1, last_connected = $2 WHERE agent_id = $3")
        .bind(&next_status)
        .bind(Utc::now())
        .bind(agent.agent_id)
        .execute(&state.db)
        .await?;

    agent.status = next_status.clone();
    agent.last_connected = Some(Utc::now());

    let _ = state.tx_events.send(crate::TreasuryEvent::AgentConnected {
        treasury_id: agent.treasury_id,
        agent_id: agent.agent_id,
    });
    let _ = state.tx_events.send(crate::TreasuryEvent::AgentStatusChanged {
        treasury_id: agent.treasury_id,
        agent_id: agent.agent_id,
        new_status: next_status.clone(),
    });
    if connection_phase == "COMPLETE" {
        let _ = state.tx_events.send(crate::TreasuryEvent::AgentActivated {
            treasury_id: agent.treasury_id,
            agent_id: agent.agent_id,
        });
    }

    Ok(Json(ConnectResponse {
        agent_id: agent.agent_id,
        treasury_id: agent.treasury_id,
        slot_status: next_status,
        connection_phase,
        reason_code,
        wallet_access,
        websocket_endpoint: format!("/v1/agents/ws/{}", agent.agent_id),
        websocket_token: ticket.websocket_token,
        session_token: ticket.session_token,
        expires_at: ticket.expires_at,
        coordinator_pubkey: state.config.stellar.coordinator_pubkey.clone(),
    }))
}

pub async fn refresh_ws_ticket(
    State(state): State<AppState>,
    agent_auth: AgentAuth,
    Json(_payload): Json<RefreshTicketRequest>,
) -> AppResult<Json<RefreshTicketResponse>> {
    let ticket = issue_agent_session(
        &state,
        agent_auth.agent_id,
        agent_auth.treasury_id,
        &agent_auth.agent_pubkey,
    )
    .await?;

    Ok(Json(ticket))
}

pub async fn agent_status(
    State(state): State<AppState>,
    agent_auth: AgentAuth,
    Path(agent_id): Path<Uuid>,
) -> AppResult<Json<AgentStatusResponse>> {
    if agent_id != agent_auth.agent_id {
        return Err(AppError::InvalidAgentSession);
    }

    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
         FROM agent_slots WHERE agent_id = $1"#,
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::AgentNotFound)?;

    let agent = row_to_agent_slot(&row);
    let (connection_phase, reason_code, rules) = compute_connection_status(&state, &agent).await?;
    let wallet_access = build_wallet_access(&state.db, agent.treasury_id, &rules).await?;
    let slot_status = if connection_phase == "COMPLETE" {
        if agent.status == "INACTIVE" {
            "INACTIVE".to_string()
        } else {
            "ACTIVE".to_string()
        }
    } else {
        pending_status_from_reason(reason_code.as_deref())
    };

    Ok(Json(AgentStatusResponse {
        agent_id: agent.agent_id,
        treasury_id: agent.treasury_id,
        slot_status,
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
    agent_auth: AgentAuth,
    Path(agent_id): Path<Uuid>,
) -> AppResult<axum::http::StatusCode> {
    if agent_id != agent_auth.agent_id {
        return Err(AppError::InvalidAgentSession);
    }

    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
         FROM agent_slots WHERE agent_id = $1"#,
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::AgentNotFound)?;
    let agent = row_to_agent_slot(&row);
    let (connection_phase, reason_code, _) = compute_connection_status(&state, &agent).await?;
    let next_status = if connection_phase == "COMPLETE" {
        "ACTIVE".to_string()
    } else {
        pending_status_from_reason(reason_code.as_deref())
    };

    sqlx::query("UPDATE agent_slots SET status = $1, last_connected = $2 WHERE agent_id = $3")
        .bind(&next_status)
        .bind(Utc::now())
        .bind(agent_id)
        .execute(&state.db)
        .await?;

    Ok(axum::http::StatusCode::OK)
}

pub async fn suspend_agent(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path((treasury_id, agent_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<AgentSlot>> {
    sqlx::query(
        "UPDATE agent_slots SET status = 'SUSPENDED', suspended_at = $1 WHERE agent_id = $2 AND treasury_id = $3",
    )
    .bind(Utc::now())
    .bind(agent_id)
    .bind(treasury_id)
    .execute(&state.db)
    .await?;

    let _ = state.tx_events.send(crate::TreasuryEvent::AgentSuspended {
        treasury_id,
        agent_id,
    });

    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
         FROM agent_slots WHERE agent_id = $1"#,
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
        "UPDATE agent_slots SET status = 'REVOKED', revoked_at = $1 WHERE agent_id = $2 AND treasury_id = $3",
    )
    .bind(Utc::now())
    .bind(agent_id)
    .bind(treasury_id)
    .execute(&state.db)
    .await?;

    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, description, wallet_address, agent_pubkey, status,
           allocation_pct::float8, tier_limit_usd::float8, concurrent_permit_cap, created_at, last_connected
         FROM agent_slots WHERE agent_id = $1"#,
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(row_to_agent_slot(&row)))
}

pub async fn agent_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(agent_id): Path<Uuid>,
    Query(query): Query<WsAuthQuery>,
) -> Result<impl IntoResponse, AppError> {
    let token = query.token.ok_or(AppError::InvalidAgentSession)?;
    let session = load_agent_session(&token, &state).await?;
    if session.agent_id != agent_id {
        return Err(AppError::InvalidAgentSession);
    }

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state, agent_id)))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, agent_id: Uuid) {
    info!(agent = %agent_id, "Agent WebSocket connected");

    let agent_info = match sqlx::query("SELECT treasury_id FROM agent_slots WHERE agent_id = $1")
        .bind(agent_id)
        .fetch_one(&state.db)
        .await
    {
        Ok(row) => row,
        Err(_) => {
            warn!(agent = %agent_id, "Agent not found for WS");
            return;
        }
    };

    let treasury_id: Uuid = agent_info.get("treasury_id");
    let wallet_allocations: std::collections::HashMap<String, f64> =
        load_constitution_rules_for_agent(&state.db, treasury_id, agent_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|rule| (rule.wallet_address, rule.allocation_pct))
            .collect();

    let mut rx = state.tx_events.subscribe();

    loop {
        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) if text == "ping" => {
                        let _ = socket.send(Message::Text("pong".to_string())).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(crate::TreasuryEvent::WalletBalanceUpdate { treasury_id: tid, wallet_address, amount, asset_code }) if tid == treasury_id => {
                        let Some(allocation_pct) = wallet_allocations.get(&wallet_address) else {
                            continue;
                        };
                        let payload = serde_json::json!({
                            "type": "WALLET_AUM_UPDATE",
                            "wallet_address": wallet_address,
                            "new_aum_usd": format!("{:.2}", amount),
                            "agent_new_max_usd": format!("{:.2}", amount * (allocation_pct / 100.0)),
                            "asset_code": asset_code
                        });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::ConstitutionUpdate { treasury_id: tid, version }) if tid == treasury_id => {
                        let payload = serde_json::json!({ "type": "CONSTITUTION_UPDATED", "version": version });
                        if let Ok(json) = serde_json::to_string(&payload) {
                            let _ = socket.send(Message::Text(json)).await;
                        }
                    }
                    Ok(crate::TreasuryEvent::PermitIssued { treasury_id: tid, agent_id: aid, permit_id, wallet_address, approved_amount }) if tid == treasury_id && aid == agent_id => {
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
                    _ => {}
                }
            }
        }
    }

    info!(agent = %agent_id, "Agent WebSocket disconnected");
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/slots", post(create_agent_from_body))
        .route("/:treasury_id", get(list_agents).post(create_agent))
        .route("/:agent_id/enroll/challenge", post(start_pubkey_enrollment))
        .route("/:agent_id/enroll-pubkey", post(enroll_pubkey))
        .route("/:treasury_id/:agent_id/suspend", post(suspend_agent))
        .route("/:treasury_id/:agent_id/revoke", post(revoke_agent))
        .route("/connect/challenge", post(connect_challenge))
        .route("/connect/complete", post(connect_complete))
        .route("/connect/status/:agent_id", get(agent_connect_status))
        .route("/ws-ticket/refresh", post(refresh_ws_ticket))
        .route("/:agent_id/status", get(agent_status))
        .route("/:agent_id/heartbeat", post(agent_heartbeat))
        .route("/ws/:agent_id", get(agent_ws))
}
