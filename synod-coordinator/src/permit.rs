use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use bigdecimal::BigDecimal;
use chrono::{Duration, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::info;
use uuid::Uuid;

use crate::auth::{verify_signed_request, AgentAuth, SignedRequestAuth};
use crate::constitution::{
    normalize_constitution_value, AgentWalletRule as CoordinatorAgentWalletRule,
};
use crate::error::{AppError, AppResult};
use crate::policy::run_policy_engine;
use crate::AppState;
use synod_shared::models::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermitGroupRequest {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub legs: Vec<PermitRequest>,
    pub require_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedPermitRequest {
    #[serde(flatten)]
    pub permit: PermitRequest,
    pub request_auth: SignedRequestAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedPermitGroupRequest {
    #[serde(flatten)]
    pub group: PermitGroupRequest,
    pub request_auth: SignedRequestAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermitDecisionResponse {
    pub permit_id: Uuid,
    #[serde(flatten)]
    pub result: PolicyResult,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/request", post(request_permit))
        .route("/group/request", post(request_permit_group))
        .route("/:id/cosign", post(cosign_permit))
        .route("/:id/outcome", post(report_outcome))
}

pub async fn request_permit(
    State(state): State<AppState>,
    agent_auth: AgentAuth,
    Json(payload): Json<SignedPermitRequest>,
) -> AppResult<(StatusCode, Json<PermitDecisionResponse>)> {
    info!(
        "Handling permit request for agent: {}",
        payload.permit.agent_id
    );
    if payload.permit.agent_id != agent_auth.agent_id
        || payload.permit.treasury_id != agent_auth.treasury_id
    {
        return Err(AppError::InvalidAgentSession);
    }
    verify_signed_request(
        &state,
        &agent_auth,
        "permit.request",
        &payload.permit,
        &payload.request_auth,
    )
    .await?;
    let mut tx = state.db.begin().await?;

    let group_id = Uuid::new_v4();
    let requested_usd = payload
        .permit
        .requested_amount
        .to_string()
        .parse::<f64>()
        .unwrap_or(0.0);

    sqlx::query(
        r#"INSERT INTO permit_groups (group_id, agent_id, treasury_id, total_requested_usd, total_approved_usd, status, expires_at)
           VALUES ($1, $2, $3, $4, $5, 'PENDING', $6)"#
    )
    .bind(group_id)
    .bind(payload.permit.agent_id)
    .bind(payload.permit.treasury_id)
    .bind(requested_usd)
    .bind(0.0)
    .bind(Utc::now() + Duration::hours(1))
    .execute(&mut *tx).await?;

    let (result, permit_id) =
        process_single_permit(&mut tx, &state, &payload.permit, group_id, Uuid::new_v4()).await?;

    // Update group status if approved/denied immediately
    let group_status = if result.approved { "ACTIVE" } else { "DENIED" };
    sqlx::query(
        "UPDATE permit_groups SET status = $1, total_approved_usd = $2 WHERE group_id = $3",
    )
    .bind(group_status)
    .bind(
        result
            .approved_amount
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0),
    )
    .bind(group_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    let status_code = if result.approved {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status_code,
        Json(PermitDecisionResponse { permit_id, result }),
    ))
}

pub async fn request_permit_group(
    State(state): State<AppState>,
    agent_auth: AgentAuth,
    Json(payload): Json<SignedPermitGroupRequest>,
) -> AppResult<(StatusCode, Json<Vec<PolicyResult>>)> {
    if payload.group.agent_id != agent_auth.agent_id
        || payload.group.treasury_id != agent_auth.treasury_id
    {
        return Err(AppError::InvalidAgentSession);
    }
    verify_signed_request(
        &state,
        &agent_auth,
        "permit.group_request",
        &payload.group,
        &payload.request_auth,
    )
    .await?;
    let mut tx = state.db.begin().await?;
    let group_id = Uuid::new_v4();
    let mut results = Vec::new();
    let mut total_requested = 0.0;

    // 1. Sort legs for deadlock prevention (alphabetical by wallet)
    let mut sorted_legs = payload.group.legs.clone();
    sorted_legs.sort_by(|a, b| a.wallet_address.cmp(&b.wallet_address));

    // 2. Create Group record first to satisfy FK
    sqlx::query(
        "INSERT INTO permit_groups (group_id, agent_id, treasury_id, require_all, total_requested_usd, total_approved_usd, status, expires_at) 
         VALUES ($1, $2, $3, $4, $5, $6, 'PENDING', $7)"
    )
    .bind(group_id)
    .bind(payload.group.agent_id)
    .bind(payload.group.treasury_id)
    .bind(payload.group.require_all)
    .bind(0.0) // Initial
    .bind(0.0) // Initial
    .bind(Utc::now() + Duration::hours(1))
    .execute(&mut *tx).await?;

    // 3. Process each leg
    let mut any_denied = false;
    for leg in &sorted_legs {
        let (result, _) =
            process_single_permit(&mut tx, &state, leg, group_id, Uuid::new_v4()).await?;
        total_requested += leg
            .requested_amount
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0);

        if !result.approved {
            any_denied = true;
        }
        results.push(result);
    }

    // 4. Apply require_all logic
    if payload.group.require_all && any_denied {
        tx.rollback().await?;
        // Update results to show all denied
        for res in &mut results {
            res.approved = false;
            res.deny_reason = Some("GROUP_REQUIRE_ALL_FAILURE".into());
        }
        return Ok((StatusCode::OK, Json(results)));
    }

    let total_approved: f64 = results
        .iter()
        .map(|r| r.approved_amount.to_string().parse::<f64>().unwrap_or(0.0))
        .sum();

    // 5. Update Group status and totals
    sqlx::query("UPDATE permit_groups SET status = 'ACTIVE', total_requested_usd = $1, total_approved_usd = $2 WHERE group_id = $3")
        .bind(total_requested)
        .bind(total_approved)
        .bind(group_id)
        .execute(&mut *tx).await?;

    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(results)))
}

#[derive(Deserialize)]
pub struct CosignRequest {
    pub xdr: String, // Stellar Transaction Envelope XDR
    pub request_auth: SignedRequestAuth,
}

pub async fn cosign_permit(
    State(state): State<AppState>,
    agent_auth: AgentAuth,
    Path(permit_id): Path<Uuid>,
    Json(payload): Json<CosignRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // 1. Fetch Permit
    let permit = sqlx::query(
        "SELECT agent_id, wallet_address, asset_code, approved_amount::float8 FROM permits WHERE permit_id = $1 AND status = 'ACTIVE'"
    )
    .bind(permit_id)
    .fetch_optional(&state.db).await?
    .ok_or(AppError::NotFound("Active permit not found".into()))?;

    if permit.get::<Uuid, _>("agent_id") != agent_auth.agent_id {
        return Err(AppError::InvalidAgentSession);
    }
    verify_signed_request(
        &state,
        &agent_auth,
        "permit.cosign",
        &payload.xdr,
        &payload.request_auth,
    )
    .await?;

    let _permit_wallet: String = permit.get("wallet_address");
    let permit_asset: String = permit.get("asset_code");
    let permit_amount: f64 = permit.get("approved_amount");

    // 2. Decode and Verify XDR Envelope
    use stellar_xdr::curr::{Limits, ReadXdr, TransactionEnvelope};

    let xdr_bytes =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &payload.xdr)
            .map_err(|_| AppError::CosignFailed("Invalid XDR base64 encoding".into()))?;

    let envelope = TransactionEnvelope::from_xdr(xdr_bytes.clone(), Limits::none())
        .map_err(|e| AppError::CosignFailed(format!("XDR decode failed: {}", e)))?;

    // Extract the transaction body and validate operations
    let ops = match &envelope {
        TransactionEnvelope::Tx(v1) => &v1.tx.operations,
        TransactionEnvelope::TxV0(v0) => &v0.tx.operations,
        _ => return Err(AppError::CosignFailed("Unsupported envelope type".into())),
    };

    if ops.is_empty() {
        return Err(AppError::CosignFailed(
            "Transaction has no operations".into(),
        ));
    }

    // Verify first operation is a payment matching the permit
    let op = &ops[0];
    match &op.body {
        stellar_xdr::curr::OperationBody::Payment(payment) => {
            // Verify asset
            let tx_asset = match &payment.asset {
                stellar_xdr::curr::Asset::Native => "XLM".to_string(),
                stellar_xdr::curr::Asset::CreditAlphanum4(a4) => {
                    String::from_utf8_lossy(&a4.asset_code.0)
                        .trim_end_matches('\0')
                        .to_string()
                }
                stellar_xdr::curr::Asset::CreditAlphanum12(a12) => {
                    String::from_utf8_lossy(&a12.asset_code.0)
                        .trim_end_matches('\0')
                        .to_string()
                }
            };

            // The amount in XDR is in stroops (1 XLM = 10_000_000 stroops)
            let tx_amount_stroops: i64 = payment.amount;
            let tx_amount = tx_amount_stroops as f64 / 10_000_000.0;

            // Allow 0.01 tolerance for floating point
            let amount_diff = (tx_amount - permit_amount).abs();
            if amount_diff > 0.01 {
                return Err(AppError::CosignFailed(format!(
                    "Amount mismatch: tx={:.7} permit={:.7}",
                    tx_amount, permit_amount
                )));
            }

            if tx_asset != permit_asset {
                return Err(AppError::CosignFailed(format!(
                    "Asset mismatch: tx={} permit={}",
                    tx_asset, permit_asset
                )));
            }

            info!(
                permit = %permit_id,
                amount = %tx_amount,
                asset = %tx_asset,
                "Permit intent verified, co-signing"
            );
        }
        _ => {
            return Err(AppError::CosignFailed(
                "First operation must be a Payment".into(),
            ));
        }
    }

    // 3. Coordinator Signs (Shard 2)
    let coordinator_secret = &state.config.stellar.coordinator_secret_key;
    if coordinator_secret.is_empty() {
        return Err(AppError::CosignFailed(
            "Coordinator secret key not configured".into(),
        ));
    }

    let signature = crate::stellar::sign_transaction_hash(
        coordinator_secret,
        &state.config.stellar.network_passphrase,
        &payload.xdr,
    )?;

    // Compute transaction hash for tracking
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&xdr_bytes);
    let tx_hash = hex::encode(hasher.finalize());

    Ok(Json(serde_json::json!({
        "status": "SIGNED",
        "permit_id": permit_id,
        "signature": signature,
        "tx_hash": tx_hash
    })))
}

#[derive(Serialize, Deserialize)]
pub struct OutcomeReport {
    pub tx_hash: String,
    pub pnl_usd: BigDecimal,
    pub final_amount_units: BigDecimal,
    pub request_auth: SignedRequestAuth,
}

#[derive(Serialize)]
struct OutcomeReportSignaturePayload {
    tx_hash: String,
    pnl_usd: String,
    final_amount_units: String,
}

pub async fn report_outcome(
    State(state): State<AppState>,
    agent_auth: AgentAuth,
    Path(permit_id): Path<Uuid>,
    Json(payload): Json<OutcomeReport>,
) -> AppResult<StatusCode> {
    let mut tx = state.db.begin().await?;

    // 1. Fetch info for drawdown check
    let treasury_info = sqlx::query(
        "SELECT t.treasury_id, p.agent_id, t.peak_aum_usd::float8, t.current_aum_usd::float8, c.content
         FROM treasuries t
         JOIN permits p ON p.treasury_id = t.treasury_id
         JOIN constitution_history c ON c.treasury_id = t.treasury_id AND c.version = t.constitution_version
         WHERE p.permit_id = $1"
    )
    .bind(permit_id)
    .fetch_one(&mut *tx).await?;

    let treasury_id: Uuid = treasury_info.get("treasury_id");
    let permit_agent_id: Uuid = treasury_info.get("agent_id");
    if permit_agent_id != agent_auth.agent_id {
        return Err(AppError::InvalidAgentSession);
    }
    let signature_payload = OutcomeReportSignaturePayload {
        tx_hash: payload.tx_hash.clone(),
        pnl_usd: payload.pnl_usd.to_string(),
        final_amount_units: payload.final_amount_units.to_string(),
    };
    verify_signed_request(
        &state,
        &agent_auth,
        "permit.outcome",
        &signature_payload,
        &payload.request_auth,
    )
    .await?;
    let peak_aum: f64 = treasury_info.get("peak_aum_usd");
    let current_aum: f64 = treasury_info.get("current_aum_usd");
    let content_json: serde_json::Value = treasury_info.get("content");
    let constitution = normalize_constitution_value(content_json)?;
    let max_drawdown = constitution.treasury_rules.max_drawdown_pct;
    let pnl: f64 = payload.pnl_usd.to_string().parse::<f64>().unwrap_or(0.0);

    let new_aum = current_aum + pnl;

    // 2. Update Permit
    sqlx::query(
        "UPDATE permits SET status = 'CONSUMED', tx_hash = $1, pnl_usd = $2, consumed_at = NOW() WHERE permit_id = $3"
    )
    .bind(&payload.tx_hash)
    .bind(pnl)
    .bind(permit_id)
    .execute(&mut *tx).await?;

    // 3. Update Treasury AUM
    sqlx::query(
        "UPDATE treasuries SET current_aum_usd = $1, updated_at = NOW() WHERE treasury_id = $2",
    )
    .bind(new_aum)
    .bind(treasury_id)
    .execute(&mut *tx)
    .await?;

    // 4. Check Drawdown Trigger
    let mut triggered_halt = false;
    if peak_aum > 0.0 {
        let drawdown_pct = (peak_aum - new_aum) / peak_aum * 100.0;
        if drawdown_pct >= max_drawdown {
            info!(
                "CRITICAL: Drawdown threshold reached ({:.2}%) for treasury {}. Triggering Halt.",
                drawdown_pct, treasury_id
            );
            crate::treasury::apply_halt(&mut tx, treasury_id).await?;
            triggered_halt = true;
        }
    }

    tx.commit().await?;
    if triggered_halt {
        let _ = state
            .tx_events
            .send(crate::TreasuryEvent::TreasuryHalted { treasury_id });
    }
    info!(
        "Outcome reported for permit {}: PnL {}",
        permit_id, payload.pnl_usd
    );
    Ok(StatusCode::OK)
}

fn build_access_from_rule(
    rule: &CoordinatorAgentWalletRule,
    can_execute: bool,
) -> AgentWalletAccess {
    AgentWalletAccess {
        agent_id: rule.agent_id,
        wallet_address: rule.wallet_address.clone(),
        allocation_pct: BigDecimal::try_from(rule.allocation_pct).unwrap_or(BigDecimal::from(100)),
        tier_limit_usd: BigDecimal::try_from(rule.tier_limit_usd).unwrap_or_default(),
        concurrent_permit_cap: rule.concurrent_permit_cap,
        can_execute,
    }
}

fn build_shared_constitution(
    treasury_id: Uuid,
    version: i32,
    content: &crate::constitution::ConstitutionContent,
) -> Constitution {
    Constitution {
        treasury_id,
        version,
        memo: content.memo.clone(),
        treasury_rules: TreasuryRules {
            max_drawdown_pct: BigDecimal::try_from(content.treasury_rules.max_drawdown_pct)
                .unwrap_or(BigDecimal::from(15)),
            max_concurrent_permits: content.treasury_rules.max_concurrent_permits,
        },
        agent_wallet_rules: content
            .agent_wallet_rules
            .iter()
            .map(|rule| AgentWalletRule {
                agent_id: rule.agent_id,
                wallet_address: rule.wallet_address.clone(),
                allocation_pct: BigDecimal::try_from(rule.allocation_pct)
                    .unwrap_or(BigDecimal::from(100)),
                tier_limit_usd: BigDecimal::try_from(rule.tier_limit_usd).unwrap_or_default(),
                concurrent_permit_cap: rule.concurrent_permit_cap,
            })
            .collect(),
    }
}

pub(crate) async fn process_single_permit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    _state: &AppState,
    payload: &PermitRequest,
    group_id: Uuid,
    leg_id: Uuid,
) -> AppResult<(PolicyResult, Uuid)> {
    // a. Agent slot status
    let agent_slot = match sqlx::query(
        "SELECT status FROM agent_slots WHERE agent_id = $1 AND treasury_id = $2",
    )
    .bind(payload.agent_id)
    .bind(payload.treasury_id)
    .fetch_optional(&mut **tx)
    .await?
    {
        Some(row) => row,
        None => {
            tracing::error!("Agent {} not found", payload.agent_id);
            return Err(AppError::AgentNotFound);
        }
    };

    let agent_status: String = agent_slot.get("status");
    let can_execute = agent_status == "ACTIVE";

    // b. Treasury Info + normalized constitution
    let treasury_info = match sqlx::query(
        "SELECT t.health, t.peak_aum_usd::float8, t.current_aum_usd::float8, t.constitution_version, c.content
         FROM treasuries t
         JOIN constitution_history c ON c.treasury_id = t.treasury_id AND c.version = t.constitution_version
         WHERE t.treasury_id = $1"
    )
    .bind(payload.treasury_id)
    .fetch_optional(&mut **tx).await? {
        Some(r) => r,
        None => {
            tracing::error!("Treasury {} not found", payload.treasury_id);
            return Err(AppError::InvalidInput("Treasury not found".into()));
        }
    };

    let constitution_content = normalize_constitution_value(treasury_info.get("content"))?;
    let wallet_rule = constitution_content
        .agent_wallet_rules
        .iter()
        .find(|rule| {
            rule.agent_id == payload.agent_id && rule.wallet_address == payload.wallet_address
        })
        .cloned()
        .ok_or_else(|| AppError::InvalidInput("Wallet not assigned to this agent".into()))?;
    let agent_access = build_access_from_rule(&wallet_rule, can_execute);

    // d. Agent's Active Reservations & Stats
    let total_active_res_f64: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(approved_amount), 0)::float8 FROM permits WHERE agent_id = $1 AND wallet_address = $2 AND status = 'ACTIVE'"
    )
    .bind(payload.agent_id)
    .bind(&payload.wallet_address)
    .fetch_one(&mut **tx).await?;
    let total_active_res = total_active_res_f64
        .to_string()
        .parse::<BigDecimal>()
        .unwrap_or_default();

    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM permits WHERE agent_id = $1 AND wallet_address = $2 AND status = 'ACTIVE'"
    )
    .bind(payload.agent_id)
    .bind(&payload.wallet_address)
    .fetch_one(&mut **tx).await?;

    let treasury_active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM permits WHERE treasury_id = $1 AND status = 'ACTIVE'",
    )
    .bind(payload.treasury_id)
    .fetch_one(&mut **tx)
    .await?;

    let health_str = treasury_info.get::<String, _>("health");
    let health = if health_str == "HALTED" {
        TreasuryHealth::Halted
    } else {
        TreasuryHealth::Healthy
    };

    let treasury_state = TreasuryState {
        treasury_id: payload.treasury_id,
        health,
        peak_aum_usd: treasury_info
            .get::<f64, _>("peak_aum_usd")
            .to_string()
            .parse::<BigDecimal>()
            .unwrap_or_default(),
        current_aum_usd: treasury_info
            .get::<f64, _>("current_aum_usd")
            .to_string()
            .parse::<BigDecimal>()
            .unwrap_or_default(),
        state_hash: "state_hash".into(),
    };

    let constitution = build_shared_constitution(
        payload.treasury_id,
        treasury_info.get::<i32, _>("constitution_version"),
        &constitution_content,
    );

    // f. Run Engine
    let result = run_policy_engine(
        payload,
        &treasury_state,
        &agent_access,
        &constitution,
        &total_active_res,
        active_count as i32,
        treasury_active_count as i32,
    );

    // g. Persist
    let permit_id = Uuid::new_v4();
    let status = if result.approved { "ACTIVE" } else { "DENIED" };

    sqlx::query(
        r#"INSERT INTO permits (permit_id, group_id, leg_id, agent_id, treasury_id, wallet_address, asset_code, 
           requested_amount, approved_amount, status, deny_reason, policy_check_number, state_snapshot_hash, coordinator_sig, expires_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#
    )
    .bind(permit_id)
    .bind(group_id)
    .bind(leg_id)
    .bind(payload.agent_id)
    .bind(payload.treasury_id)
    .bind(payload.wallet_address.clone())
    .bind(payload.asset_code.clone())
    .bind(payload.requested_amount.to_string().parse::<f64>().unwrap_or(0.0))
    .bind(result.approved_amount.to_string().parse::<f64>().unwrap_or(0.0))
    .bind(status)
    .bind(result.deny_reason.clone())
    .bind(result.policy_check_number)
    .bind("snap")
    .bind("sig")
    .bind(Utc::now() + Duration::hours(1))
    .execute(&mut **tx).await?;

    Ok((result, permit_id))
}
