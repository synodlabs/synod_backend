use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
use std::str::FromStr;
use uuid::Uuid;

use crate::{
    auth::{load_agent_session, store_agent_session, AgentSession},
    constitution::{normalize_constitution_value, rules_for_agent},
    error::{AppError, AppResult},
    permit::process_single_permit,
    stellar, AppState,
};
use synod_shared::models::PermitRequest;

const MCP_CONNECT_TTL_SECS: u64 = 120;
const MCP_WS_TTL_SECS: u64 = 3600;
const MCP_INTENT_TTL_SECS: u64 = 86_400;

#[derive(Debug, Deserialize)]
pub struct ConnectInitRequest {
    pub public_key: String,
}

#[derive(Debug, Serialize)]
pub struct ConnectInitResponse {
    pub nonce: String,
    pub expires_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct ConnectCompleteRequest {
    pub public_key: String,
    pub signature: String,
    pub nonce: String,
}

#[derive(Debug, Serialize)]
pub struct ConnectCompleteResponse {
    pub ws_ticket: String,
    pub agent_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct ConnectStatusQuery {
    pub public_key: String,
}

#[derive(Debug, Serialize)]
pub struct ConnectStatusResponse {
    pub status: String,
    pub agent_id: Option<Uuid>,
    pub reason_code: Option<String>,
    pub connect_allowed: bool,
}

#[derive(Debug, Deserialize)]
pub struct PolicyQuery {
    pub public_key: String,
}

#[derive(Debug, Serialize)]
pub struct PolicyResponse {
    pub agent_id: Uuid,
    pub public_key: String,
    pub rules: Vec<Value>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct SubmitIntentRequest {
    pub public_key: String,
    pub signature: String,
    pub intent: Value,
}

#[derive(Debug, Serialize)]
pub struct SubmitIntentResponse {
    pub intent_id: Uuid,
    pub status: String,
    pub tx_hash: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentWsQuery {
    pub ticket: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConnectNonceRecord {
    agent_id: Uuid,
    treasury_id: Uuid,
    public_key: String,
    nonce: String,
    expires_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct IntentRecord {
    intent_id: Uuid,
    agent_id: Uuid,
    treasury_id: Uuid,
    public_key: String,
    intent_type: String,
    status: String,
    reason: Option<String>,
    tx_hash: Option<String>,
    permit_id: Option<Uuid>,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug)]
struct McpAgentRecord {
    agent_id: Uuid,
    treasury_id: Uuid,
    public_key: String,
    status: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct ResolvedIntent {
    kind: String,
    amount: BigDecimal,
    asset_code: String,
    wallet_address: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/connect/init", post(connect_init))
        .route("/connect/complete", post(connect_complete))
        .route("/connect/status", get(connect_status))
        .route("/policy", get(get_policy))
        .route("/intents/submit", post(submit_intent))
        .route("/agent/ws", get(agent_ws))
}

pub async fn connect_status(
    State(state): State<AppState>,
    Query(query): Query<ConnectStatusQuery>,
) -> AppResult<Json<ConnectStatusResponse>> {
    validate_public_key(&query.public_key)?;

    let Some(agent) = load_agent_by_public_key(&state, &query.public_key).await? else {
        return Ok(Json(ConnectStatusResponse {
            status: "not_found".to_string(),
            agent_id: None,
            reason_code: None,
            connect_allowed: false,
        }));
    };

    let (connect_allowed, reason_code) = connect_allowed_and_reason(&agent);

    Ok(Json(ConnectStatusResponse {
        status: "ready".to_string(),
        agent_id: Some(agent.agent_id),
        reason_code,
        connect_allowed,
    }))
}

pub async fn connect_init(
    State(state): State<AppState>,
    Json(payload): Json<ConnectInitRequest>,
) -> AppResult<Json<ConnectInitResponse>> {
    validate_public_key(&payload.public_key)?;
    let agent = load_agent_by_public_key(&state, &payload.public_key)
        .await?
        .ok_or(AppError::AgentNotFound)?;

    ensure_connect_allowed(&agent)?;

    let nonce = Uuid::new_v4().to_string();
    let expires_at = Utc::now() + chrono::Duration::seconds(MCP_CONNECT_TTL_SECS as i64);
    let record = ConnectNonceRecord {
        agent_id: agent.agent_id,
        treasury_id: agent.treasury_id,
        public_key: agent.public_key.clone(),
        nonce: nonce.clone(),
        expires_at: expires_at.timestamp(),
    };

    let mut redis = state.redis.clone();
    let encoded = serde_json::to_string(&record)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("Nonce encode failed: {}", error)))?;

    let _: () = redis
        .set_ex(
            mcp_connect_nonce_key(&agent.public_key),
            encoded,
            MCP_CONNECT_TTL_SECS,
        )
        .await
        .map_err(AppError::Redis)?;

    Ok(Json(ConnectInitResponse {
        nonce,
        expires_at: expires_at.timestamp_millis(),
    }))
}

pub async fn connect_complete(
    State(state): State<AppState>,
    Json(payload): Json<ConnectCompleteRequest>,
) -> AppResult<Json<ConnectCompleteResponse>> {
    validate_public_key(&payload.public_key)?;

    let mut redis = state.redis.clone();
    let stored: Option<String> = redis
        .get(mcp_connect_nonce_key(&payload.public_key))
        .await
        .map_err(AppError::Redis)?;
    let stored = stored.ok_or(AppError::ChallengeExpired)?;
    let record: ConnectNonceRecord = serde_json::from_str(&stored)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("Nonce decode failed: {}", error)))?;

    if record.public_key != payload.public_key
        || record.nonce != payload.nonce
        || record.expires_at <= Utc::now().timestamp()
    {
        return Err(AppError::ChallengeExpired);
    }

    let connect_hash = stellar::sha256_bytes(&canonical_json_bytes(&serde_json::json!({
        "action": "connect",
        "domain": "synod",
        "nonce": payload.nonce,
    })));
    stellar::verify_raw_ed25519_signature(&payload.public_key, &connect_hash, &payload.signature)?;

    let _: redis::RedisResult<()> = redis.del(mcp_connect_nonce_key(&payload.public_key)).await;

    let agent = load_agent_by_public_key(&state, &payload.public_key)
        .await?
        .ok_or(AppError::AgentNotFound)?;
    ensure_connect_allowed(&agent)?;

    let ws_ticket = issue_ws_ticket(&state, &agent).await?;

    sqlx::query("UPDATE agent_slots SET last_connected = $1 WHERE agent_id = $2")
        .bind(Utc::now())
        .bind(agent.agent_id)
        .execute(&state.db)
        .await?;

    let _ = state.tx_events.send(crate::TreasuryEvent::AgentConnected {
        treasury_id: agent.treasury_id,
        agent_id: agent.agent_id,
    });

    Ok(Json(ConnectCompleteResponse {
        ws_ticket,
        agent_id: agent.agent_id,
    }))
}

pub async fn get_policy(
    State(state): State<AppState>,
    Query(query): Query<PolicyQuery>,
) -> AppResult<Json<PolicyResponse>> {
    validate_public_key(&query.public_key)?;
    let agent = load_agent_by_public_key(&state, &query.public_key)
        .await?
        .ok_or(AppError::AgentNotFound)?;

    let (rules, updated_at) = project_policy_rules(&state, &agent).await?;

    Ok(Json(PolicyResponse {
        agent_id: agent.agent_id,
        public_key: agent.public_key,
        rules,
        created_at: agent.created_at.timestamp_millis(),
        updated_at,
    }))
}

pub async fn submit_intent(
    State(state): State<AppState>,
    Json(payload): Json<SubmitIntentRequest>,
) -> AppResult<Json<SubmitIntentResponse>> {
    validate_public_key(&payload.public_key)?;

    let agent = load_agent_by_public_key(&state, &payload.public_key)
        .await?
        .ok_or(AppError::AgentNotFound)?;
    ensure_connect_allowed(&agent)?;

    let canonical_intent = canonical_json_bytes(&payload.intent);
    stellar::verify_raw_ed25519_signature(
        &payload.public_key,
        &canonical_intent,
        &payload.signature,
    )?;

    let intent_id = Uuid::new_v4();
    let created_at = Utc::now().timestamp_millis();

    let rules = load_agent_rules(&state, agent.treasury_id, agent.agent_id).await?;
    let resolved_intent = parse_and_resolve_intent(&payload.intent, &agent, &rules)?;

    let mut tx = state.db.begin().await?;
    let group_id = Uuid::new_v4();
    let requested_usd = resolved_intent
        .amount
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);

    sqlx::query(
        r#"INSERT INTO permit_groups (group_id, agent_id, treasury_id, total_requested_usd, total_approved_usd, status, expires_at)
           VALUES ($1, $2, $3, $4, $5, 'PENDING', $6)"#,
    )
    .bind(group_id)
    .bind(agent.agent_id)
    .bind(agent.treasury_id)
    .bind(requested_usd)
    .bind(0.0)
    .bind(Utc::now() + chrono::Duration::hours(1))
    .execute(&mut *tx)
    .await?;

    let permit_request = PermitRequest {
        agent_id: agent.agent_id,
        treasury_id: agent.treasury_id,
        wallet_address: resolved_intent.wallet_address.clone(),
        asset_code: resolved_intent.asset_code.clone(),
        asset_issuer: None,
        requested_amount: resolved_intent.amount.clone(),
    };

    let (result, permit_id) =
        process_single_permit(&mut tx, &state, &permit_request, group_id, Uuid::new_v4()).await?;
    let status = if result.approved {
        "confirmed"
    } else {
        "rejected"
    }
    .to_string();
    let approved_total = result
        .approved_amount
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);

    sqlx::query(
        "UPDATE permit_groups SET status = $1, total_approved_usd = $2 WHERE group_id = $3",
    )
    .bind(if result.approved { "ACTIVE" } else { "DENIED" })
    .bind(approved_total)
    .bind(group_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let reason = result.deny_reason.clone();
    let record = IntentRecord {
        intent_id,
        agent_id: agent.agent_id,
        treasury_id: agent.treasury_id,
        public_key: agent.public_key.clone(),
        intent_type: resolved_intent.kind.clone(),
        status: status.clone(),
        reason: reason.clone(),
        tx_hash: None,
        permit_id: Some(permit_id),
        created_at,
        updated_at: Utc::now().timestamp_millis(),
    };
    store_intent_record(&state, &record).await?;

    let _ = state.tx_events.send(crate::TreasuryEvent::IntentReceived {
        treasury_id: agent.treasury_id,
        agent_id: agent.agent_id,
        intent_id,
        intent_type: resolved_intent.kind.clone(),
        wallet_address: resolved_intent.wallet_address.clone(),
        asset_code: resolved_intent.asset_code.clone(),
        amount: resolved_intent.amount.to_string(),
    });

    if result.approved {
        let _ = state.tx_events.send(crate::TreasuryEvent::IntentConfirmed {
            treasury_id: agent.treasury_id,
            agent_id: agent.agent_id,
            intent_id,
            tx_hash: None,
        });
    } else {
        let _ = state.tx_events.send(crate::TreasuryEvent::IntentRejected {
            treasury_id: agent.treasury_id,
            agent_id: agent.agent_id,
            intent_id,
            reason: reason
                .clone()
                .unwrap_or_else(|| "POLICY_REJECTED".to_string()),
        });
    }

    Ok(Json(SubmitIntentResponse {
        intent_id,
        status,
        tx_hash: None,
        reason,
    }))
}

pub async fn agent_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(query): Query<AgentWsQuery>,
) -> AppResult<impl IntoResponse> {
    let session = load_agent_session(&query.ticket, &state).await?;
    Ok(ws.on_upgrade(move |socket| handle_agent_socket(socket, state, session)))
}

async fn handle_agent_socket(mut socket: WebSocket, state: AppState, session: AgentSession) {
    let mut rx = state.tx_events.subscribe();

    loop {
        tokio::select! {
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) if text == "ping" => {
                        let _ = socket.send(Message::Text("pong".to_string())).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(event) => {
                        if let Some(payload) = map_event_for_mcp(&event, &session) {
                            if let Ok(json) = serde_json::to_string(&payload) {
                                if socket.send(Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        }
    }
}

fn map_event_for_mcp(event: &crate::TreasuryEvent, session: &AgentSession) -> Option<Value> {
    match event {
        crate::TreasuryEvent::ConstitutionUpdate {
            treasury_id,
            version,
        } if *treasury_id == session.treasury_id => Some(serde_json::json!({
            "type": "policy_updated",
            "version": version,
        })),
        crate::TreasuryEvent::AgentSuspended {
            treasury_id,
            agent_id,
        } if *treasury_id == session.treasury_id && *agent_id == session.agent_id => {
            Some(serde_json::json!({ "type": "agent_suspended" }))
        }
        crate::TreasuryEvent::IntentConfirmed {
            treasury_id,
            agent_id,
            intent_id,
            tx_hash,
        } if *treasury_id == session.treasury_id && *agent_id == session.agent_id => {
            Some(serde_json::json!({
                "type": "intent_confirmed",
                "intent_id": intent_id,
                "tx_hash": tx_hash,
            }))
        }
        crate::TreasuryEvent::IntentReceived {
            treasury_id,
            agent_id,
            intent_id,
            intent_type,
            wallet_address,
            asset_code,
            amount,
        } if *treasury_id == session.treasury_id && *agent_id == session.agent_id => {
            Some(serde_json::json!({
                "type": "intent_received",
                "intent_id": intent_id,
                "intent_type": intent_type,
                "wallet_address": wallet_address,
                "asset_code": asset_code,
                "amount": amount,
            }))
        }
        crate::TreasuryEvent::IntentRejected {
            treasury_id,
            agent_id,
            intent_id,
            reason,
        } if *treasury_id == session.treasury_id && *agent_id == session.agent_id => {
            Some(serde_json::json!({
                "type": "intent_rejected",
                "intent_id": intent_id,
                "reason": reason,
            }))
        }
        crate::TreasuryEvent::IntentFailed {
            treasury_id,
            agent_id,
            intent_id,
            reason,
        } if *treasury_id == session.treasury_id && *agent_id == session.agent_id => {
            Some(serde_json::json!({
                "type": "intent_failed",
                "intent_id": intent_id,
                "reason": reason,
            }))
        }
        _ => None,
    }
}

async fn load_agent_by_public_key(
    state: &AppState,
    public_key: &str,
) -> AppResult<Option<McpAgentRecord>> {
    let row = sqlx::query(
        r#"SELECT agent_id, treasury_id, name, agent_pubkey, status, created_at, last_connected
           FROM agent_slots
           WHERE agent_pubkey = $1
           ORDER BY created_at DESC
           LIMIT 1"#,
    )
    .bind(public_key)
    .fetch_optional(&state.db)
    .await?;

    Ok(row.map(|row| McpAgentRecord {
        agent_id: row.get("agent_id"),
        treasury_id: row.get("treasury_id"),
        public_key: row.get("agent_pubkey"),
        status: row.get("status"),
        created_at: row.get("created_at"),
    }))
}

fn connect_allowed_and_reason(agent: &McpAgentRecord) -> (bool, Option<String>) {
    match agent.status.as_str() {
        "SUSPENDED" => (false, Some("AGENT_SUSPENDED".to_string())),
        "REVOKED" => (false, Some("AGENT_REVOKED".to_string())),
        _ => (true, None),
    }
}

fn ensure_connect_allowed(agent: &McpAgentRecord) -> AppResult<()> {
    match agent.status.as_str() {
        "SUSPENDED" => Err(AppError::AgentSuspended),
        "REVOKED" => Err(AppError::AgentRevoked),
        _ => Ok(()),
    }
}

async fn issue_ws_ticket(state: &AppState, agent: &McpAgentRecord) -> AppResult<String> {
    let ticket = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let expires_at = Utc::now() + chrono::Duration::seconds(MCP_WS_TTL_SECS as i64);
    let session = AgentSession {
        agent_id: agent.agent_id,
        treasury_id: agent.treasury_id,
        agent_pubkey: agent.public_key.clone(),
        issued_at: Utc::now().timestamp(),
        expires_at: expires_at.timestamp(),
    };

    store_agent_session(state, &ticket, &session, MCP_WS_TTL_SECS).await?;
    Ok(ticket)
}

async fn project_policy_rules(
    state: &AppState,
    agent: &McpAgentRecord,
) -> AppResult<(Vec<Value>, i64)> {
    let row = sqlx::query(
        r#"SELECT version, content, executed_at
           FROM constitution_history
           WHERE treasury_id = $1
           ORDER BY version DESC
           LIMIT 1"#,
    )
    .bind(agent.treasury_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(row) = row else {
        return Ok((Vec::new(), agent.created_at.timestamp_millis()));
    };

    let executed_at: DateTime<Utc> = row.get("executed_at");
    let content = normalize_constitution_value(row.get("content"))?;
    let agent_rules = rules_for_agent(&content, agent.agent_id);

    let mut projected = Vec::with_capacity(agent_rules.len() + 1);
    projected.push(serde_json::json!({
        "type": "treasury_guard",
        "max_drawdown_pct": content.treasury_rules.max_drawdown_pct.to_string(),
        "max_concurrent_permits": content.treasury_rules.max_concurrent_permits,
    }));

    for rule in agent_rules {
        projected.push(serde_json::json!({
            "type": "wallet_access",
            "wallet_address": rule.wallet_address,
            "allocation_pct": decimal_string(rule.allocation_pct),
            "tier_limit_usd": decimal_string(rule.tier_limit_usd),
            "concurrent_permit_cap": rule.concurrent_permit_cap,
        }));
    }

    Ok((projected, executed_at.timestamp_millis()))
}

async fn load_agent_rules(
    state: &AppState,
    treasury_id: Uuid,
    agent_id: Uuid,
) -> AppResult<Vec<crate::constitution::AgentWalletRule>> {
    let content_json: Option<Value> = sqlx::query_scalar(
        "SELECT content FROM constitution_history WHERE treasury_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(treasury_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(content_json) = content_json else {
        return Ok(Vec::new());
    };

    let content = normalize_constitution_value(content_json)?;
    Ok(rules_for_agent(&content, agent_id))
}

fn parse_and_resolve_intent(
    intent: &Value,
    agent: &McpAgentRecord,
    rules: &[crate::constitution::AgentWalletRule],
) -> AppResult<ResolvedIntent> {
    let kind = intent
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidInput("Intent type is required".to_string()))?
        .to_string();

    let amount_text = intent
        .get("amount")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::InvalidInput("Intent amount must be a string".to_string()))?;
    let amount = BigDecimal::from_str(amount_text).map_err(|_| {
        AppError::InvalidInput("Intent amount must be a valid decimal string".to_string())
    })?;

    let destination = intent
        .get("to")
        .or_else(|| intent.get("destination"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let asset_code = match kind.as_str() {
        "payment" | "delegate" => intent
            .get("asset")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::InvalidInput(format!("{} intents require an asset field", kind))
            })?
            .to_string(),
        "swap" => intent
            .get("from_asset")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidInput("swap intents require from_asset".to_string()))?
            .to_string(),
        other => {
            return Err(AppError::InvalidInput(format!(
                "Unsupported intent type '{}'. Expected payment, swap, or delegate.",
                other
            )))
        }
    };

    if matches!(kind.as_str(), "payment" | "delegate") && destination.is_none() {
        return Err(AppError::InvalidInput(format!(
            "{} intents require 'to' or 'destination'",
            kind
        )));
    }

    if kind == "swap" && intent.get("to_asset").and_then(Value::as_str).is_none() {
        return Err(AppError::InvalidInput(
            "swap intents require to_asset".to_string(),
        ));
    }

    let wallet_address = resolve_wallet_address(intent, rules)?;

    let _ = agent;
    Ok(ResolvedIntent {
        kind,
        amount,
        asset_code,
        wallet_address,
    })
}

fn resolve_wallet_address(
    intent: &Value,
    rules: &[crate::constitution::AgentWalletRule],
) -> AppResult<String> {
    let explicit_wallet = intent
        .get("wallet_address")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(wallet_address) = explicit_wallet {
        let allowed = rules
            .iter()
            .any(|rule| rule.wallet_address == wallet_address);
        if !allowed {
            return Err(AppError::InvalidInput(
                "The supplied wallet_address is not assigned to this agent".to_string(),
            ));
        }
        return Ok(wallet_address.to_string());
    }

    match rules {
        [] => Err(AppError::InvalidInput(
            "No eligible wallet is assigned to this agent yet. Assign a wallet in Policy first.".to_string(),
        )),
        [rule] => Ok(rule.wallet_address.clone()),
        _ => Err(AppError::InvalidInput(
            "Multiple eligible wallets are assigned to this agent. Include wallet_address in the signed intent.".to_string(),
        )),
    }
}

async fn store_intent_record(state: &AppState, record: &IntentRecord) -> AppResult<()> {
    let mut redis = state.redis.clone();
    let encoded = serde_json::to_string(record)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("Intent encode failed: {}", error)))?;
    let _: () = redis
        .set_ex(
            mcp_intent_key(record.intent_id),
            encoded,
            MCP_INTENT_TTL_SECS,
        )
        .await
        .map_err(AppError::Redis)?;
    Ok(())
}

fn mcp_connect_nonce_key(public_key: &str) -> String {
    format!("mcp:connect:nonce:{}", public_key)
}

fn mcp_intent_key(intent_id: Uuid) -> String {
    format!("mcp:intent:{}", intent_id)
}

fn validate_public_key(public_key: &str) -> AppResult<()> {
    let trimmed = public_key.trim();
    if trimmed.len() != 56 || !trimmed.starts_with('G') {
        return Err(AppError::InvalidInput(
            "Invalid Stellar public key format".to_string(),
        ));
    }
    Ok(())
}

fn decimal_string(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{:.0}", value)
    } else {
        value.to_string()
    }
}

fn canonical_json_bytes(value: &Value) -> Vec<u8> {
    canonical_json(value).into_bytes()
}

fn canonical_json(value: &Value) -> String {
    serialize_canonical_value(value)
}

fn serialize_canonical_value(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
        }
        Value::Array(values) => {
            let items = values
                .iter()
                .map(serialize_canonical_value)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{}]", items)
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let items = entries
                .into_iter()
                .map(|(key, nested)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string()),
                        serialize_canonical_value(nested)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{}}}", items)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_json, connect_allowed_and_reason, map_event_for_mcp, parse_and_resolve_intent,
        McpAgentRecord,
    };
    use chrono::Utc;
    use uuid::Uuid;

    #[test]
    fn canonical_json_sorts_nested_keys() {
        let value = serde_json::json!({
            "z": 1,
            "a": {
                "d": true,
                "b": "x"
            }
        });

        assert_eq!(canonical_json(&value), r#"{"a":{"b":"x","d":true},"z":1}"#);
    }

    #[test]
    fn ready_projection_keeps_slot_ready_but_marks_connect_blocked() {
        let agent = McpAgentRecord {
            agent_id: Uuid::new_v4(),
            treasury_id: Uuid::new_v4(),
            public_key: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            status: "SUSPENDED".to_string(),
            created_at: Utc::now(),
        };

        let (connect_allowed, reason_code) = connect_allowed_and_reason(&agent);
        assert!(!connect_allowed);
        assert_eq!(reason_code.as_deref(), Some("AGENT_SUSPENDED"));
    }

    #[test]
    fn event_mapping_uses_mcp_event_names() {
        let treasury_id = Uuid::new_v4();
        let agent_id = Uuid::new_v4();
        let intent_id = Uuid::new_v4();
        let session = crate::auth::AgentSession {
            agent_id,
            treasury_id,
            agent_pubkey: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            issued_at: Utc::now().timestamp(),
            expires_at: (Utc::now() + chrono::Duration::hours(1)).timestamp(),
        };

        let payload = map_event_for_mcp(
            &crate::TreasuryEvent::IntentRejected {
                treasury_id,
                agent_id,
                intent_id,
                reason: "POLICY_REJECTED".to_string(),
            },
            &session,
        )
        .expect("event should map");

        assert_eq!(payload["type"], "intent_rejected");
        assert_eq!(payload["intent_id"], intent_id.to_string());
        assert_eq!(payload["reason"], "POLICY_REJECTED");
    }

    #[test]
    fn payment_intent_requires_destination_and_wallet_resolution() {
        let agent = McpAgentRecord {
            agent_id: Uuid::new_v4(),
            treasury_id: Uuid::new_v4(),
            public_key: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            status: "PENDING_CONFIGURATION".to_string(),
            created_at: Utc::now(),
        };
        let rules = vec![crate::constitution::AgentWalletRule {
            agent_id: agent.agent_id,
            wallet_address: "GWALLET".to_string(),
            allocation_pct: 100.0,
            tier_limit_usd: 1000.0,
            concurrent_permit_cap: 1,
        }];

        let error = parse_and_resolve_intent(
            &serde_json::json!({
                "type": "payment",
                "amount": "10",
                "asset": "XLM"
            }),
            &agent,
            &rules,
        )
        .expect_err("destination should be required");

        assert!(error.to_string().contains("payment intents require"));
    }

    #[test]
    fn multiple_wallets_require_explicit_wallet_address() {
        let agent_id = Uuid::new_v4();
        let agent = McpAgentRecord {
            agent_id,
            treasury_id: Uuid::new_v4(),
            public_key: "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF".to_string(),
            status: "PENDING_CONFIGURATION".to_string(),
            created_at: Utc::now(),
        };
        let rules = vec![
            crate::constitution::AgentWalletRule {
                agent_id,
                wallet_address: "GWALLET1".to_string(),
                allocation_pct: 60.0,
                tier_limit_usd: 1000.0,
                concurrent_permit_cap: 1,
            },
            crate::constitution::AgentWalletRule {
                agent_id,
                wallet_address: "GWALLET2".to_string(),
                allocation_pct: 40.0,
                tier_limit_usd: 500.0,
                concurrent_permit_cap: 1,
            },
        ];

        let error = parse_and_resolve_intent(
            &serde_json::json!({
                "type": "delegate",
                "destination": "GDEST",
                "amount": "5",
                "asset": "XLM"
            }),
            &agent,
            &rules,
        )
        .expect_err("wallet selection should be ambiguous");

        assert!(error.to_string().contains("Multiple eligible wallets"));
    }
}
