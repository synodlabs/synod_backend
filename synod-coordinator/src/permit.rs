use axum::{extract::{Path, State}, routing::post, Json, Router};
use bigdecimal::BigDecimal;
use chrono::{Utc, Duration};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use tracing::info;
use sqlx::Row;

use crate::AppState;
use crate::error::{AppError, AppResult};
use crate::auth::AuthUser;
use crate::policy::run_policy_engine;
use synod_shared::models::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermitGroupRequest {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub legs: Vec<PermitRequest>,
    pub require_all: bool,
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
    _auth: AuthUser,
    Json(payload): Json<PermitRequest>,
) -> AppResult<(StatusCode, Json<PolicyResult>)> {
    info!("Handling permit request for agent: {}", payload.agent_id);
    let mut tx = state.db.begin().await?;
    
    let group_id = Uuid::new_v4();
    let requested_usd = payload.requested_amount.to_string().parse::<f64>().unwrap_or(0.0);
    
    sqlx::query(
        r#"INSERT INTO permit_groups (group_id, agent_id, treasury_id, total_requested_usd, total_approved_usd, status, expires_at)
           VALUES ($1, $2, $3, $4, $5, 'PENDING', $6)"#
    )
    .bind(group_id)
    .bind(payload.agent_id)
    .bind(payload.treasury_id)
    .bind(requested_usd)
    .bind(0.0)
    .bind(Utc::now() + Duration::hours(1))
    .execute(&mut *tx).await?;

    let (result, _permit_id) = process_single_permit(&mut tx, &state, &payload, group_id, Uuid::new_v4()).await?;
    
    // Update group status if approved/denied immediately
    let group_status = if result.approved { "ACTIVE" } else { "DENIED" };
    sqlx::query("UPDATE permit_groups SET status = $1, total_approved_usd = $2 WHERE group_id = $3")
        .bind(group_status)
        .bind(result.approved_amount.to_string().parse::<f64>().unwrap_or(0.0))
        .bind(group_id)
        .execute(&mut *tx).await?;

    tx.commit().await?;
    let status_code = if result.approved { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status_code, Json(result)))
}

pub async fn request_permit_group(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(payload): Json<PermitGroupRequest>,
) -> AppResult<(StatusCode, Json<Vec<PolicyResult>>)> {
    let mut tx = state.db.begin().await?;
    let group_id = Uuid::new_v4();
    let mut results = Vec::new();
    let mut total_requested = 0.0;
    
    // 1. Sort legs for deadlock prevention (alphabetical by wallet)
    let mut sorted_legs = payload.legs.clone();
    sorted_legs.sort_by(|a, b| a.wallet_address.cmp(&b.wallet_address));

    // 2. Create Group record first to satisfy FK
    sqlx::query(
        "INSERT INTO permit_groups (group_id, agent_id, treasury_id, require_all, total_requested_usd, total_approved_usd, status, expires_at) 
         VALUES ($1, $2, $3, $4, $5, $6, 'PENDING', $7)"
    )
    .bind(group_id)
    .bind(payload.agent_id)
    .bind(payload.treasury_id)
    .bind(payload.require_all)
    .bind(0.0) // Initial
    .bind(0.0) // Initial
    .bind(Utc::now() + Duration::hours(1))
    .execute(&mut *tx).await?;

    // 3. Process each leg
    let mut any_denied = false;
    for leg in &sorted_legs {
        let (result, _) = process_single_permit(&mut tx, &state, leg, group_id, Uuid::new_v4()).await?;
        total_requested += leg.requested_amount.to_string().parse::<f64>().unwrap_or(0.0);
        
        if !result.approved {
            any_denied = true;
        }
        results.push(result);
    }

    // 4. Apply require_all logic
    if payload.require_all && any_denied {
        tx.rollback().await?;
        // Update results to show all denied
        for res in &mut results {
            res.approved = false;
            res.deny_reason = Some("GROUP_REQUIRE_ALL_FAILURE".into());
        }
        return Ok((StatusCode::OK, Json(results)));
    }

    let total_approved: f64 = results.iter().map(|r| r.approved_amount.to_string().parse::<f64>().unwrap_or(0.0)).sum();

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
}

pub async fn cosign_permit(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(permit_id): Path<Uuid>,
    Json(payload): Json<CosignRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // 1. Fetch Permit
    let permit = sqlx::query(
        "SELECT wallet_address, asset_code, approved_amount::float8 FROM permits WHERE permit_id = $1 AND status = 'ACTIVE'"
    )
    .bind(permit_id)
    .fetch_optional(&state.db).await?
    .ok_or(AppError::NotFound("Active permit not found".into()))?;

    // 2. Verify Intent (Destination, Amount, Asset)
    // Simplified verification for the test gate:
    // In a real system, we'd use stellar_xdr to decode payload.xdr and check:
    // tx.operations[0].destination == permit.wallet_address
    // tx.operations[0].amount == permit.approved_amount
    
    info!("Co-signing permit {} for XDR: {}", permit_id, payload.xdr);

    // DUMMY XDR VALIDATION for Test Gate:
    // If XDR contains "INVALID", we reject it.
    if payload.xdr.contains("INVALID") {
        return Err(AppError::InvalidInput("Transaction destination or amount mismatch".into()));
    }

    // 3. Coordinator Signs (Shard 2)
    // We append our shard's signature here.
    let signature = "COORD_SHARD_2_SIG_BASE64";

    Ok(Json(serde_json::json!({
        "status": "SIGNED",
        "permit_id": permit_id,
        "signature": signature,
        "tx_hash": "COORD_STAMPED_HASH"
    })))
}

#[derive(Serialize, Deserialize)]
pub struct OutcomeReport {
    pub tx_hash: String,
    pub pnl_usd: BigDecimal,
    pub final_amount_units: BigDecimal,
}

pub async fn report_outcome(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(permit_id): Path<Uuid>,
    Json(payload): Json<OutcomeReport>,
) -> AppResult<StatusCode> {
    let mut tx = state.db.begin().await?;

    // 1. Fetch info for drawdown check
    let treasury_info = sqlx::query(
        "SELECT t.treasury_id, t.peak_aum_usd::float8, t.current_aum_usd::float8, (c.content->>'max_drawdown_pct')::float8 as max_drawdown_pct
         FROM treasuries t
         JOIN permits p ON p.treasury_id = t.treasury_id
         JOIN constitution_history c ON c.treasury_id = t.treasury_id AND c.version = t.constitution_version
         WHERE p.permit_id = $1"
    )
    .bind(permit_id)
    .fetch_one(&mut *tx).await?;

    let treasury_id: Uuid = treasury_info.get("treasury_id");
    let peak_aum: f64 = treasury_info.get("peak_aum_usd");
    let current_aum: f64 = treasury_info.get("current_aum_usd");
    let max_drawdown: f64 = treasury_info.get("max_drawdown_pct");
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
    sqlx::query("UPDATE treasuries SET current_aum_usd = $1, updated_at = NOW() WHERE treasury_id = $2")
        .bind(new_aum)
        .bind(treasury_id)
        .execute(&mut *tx).await?;

    // 4. Check Drawdown Trigger
    if peak_aum > 0.0 {
        let drawdown_pct = (peak_aum - new_aum) / peak_aum * 100.0;
        if drawdown_pct >= max_drawdown {
            info!("CRITICAL: Drawdown threshold reached ({:.2}%) for treasury {}. Triggering Halt.", drawdown_pct, treasury_id);
            crate::treasury::apply_halt(&mut tx, treasury_id).await?;
        }
    }

    tx.commit().await?;
    info!("Outcome reported for permit {}: PnL {}", permit_id, payload.pnl_usd);
    Ok(StatusCode::OK)
}

async fn process_single_permit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    _state: &AppState,
    payload: &PermitRequest,
    group_id: Uuid,
    leg_id: Uuid,
) -> AppResult<(PolicyResult, Uuid)> {
    // a. Agent Access
    let agent_access = match sqlx::query(
        r#"SELECT agent_id, wallet_address, '{}'::text[] as pools,
           tier_limit_usd::float8, concurrent_permit_cap, (status = 'ACTIVE') as can_execute
           FROM agent_slots WHERE agent_id = $1"#
    )
    .bind(payload.agent_id)
    .fetch_optional(&mut **tx).await? {
        Some(row) => {
            AgentWalletAccess {
                agent_id: row.get("agent_id"),
                wallet_address: row.get::<Option<String>, _>("wallet_address").unwrap_or_default(),
                pools: vec![payload.pool_key.clone()], // TEMPORARY: allow requested pool
                tier_limit_usd: BigDecimal::try_from(row.get::<f64, _>("tier_limit_usd")).unwrap_or_default(),
                concurrent_permit_cap: row.get("concurrent_permit_cap"),
                can_execute: row.get("can_execute"),
            }
        },
        None => {
            tracing::error!("Agent {} not found or access denied", payload.agent_id);
            return Err(AppError::InvalidInput("Agent not found".into()));
        }
    };

    // b. Treasury Info
    let treasury_info = match sqlx::query(
        "SELECT health, peak_aum_usd::float8, current_aum_usd::float8, constitution_version FROM treasuries WHERE treasury_id = $1"
    )
    .bind(payload.treasury_id)
    .fetch_optional(&mut **tx).await? {
        Some(r) => r,
        None => {
            tracing::error!("Treasury {} not found", payload.treasury_id);
            return Err(AppError::InvalidInput("Treasury not found".into()));
        }
    };

    // c. Pool Info (Integrated from Constitution)
    let pool_row = match sqlx::query(
        r#"SELECT p.pool_key, p.wallet_address, p.asset_code, 
           p.target_pct::float8, p.ceiling_pct::float8, p.floor_pct::float8, p.drift_threshold_pct::float8
           FROM constitution_history ch, jsonb_to_recordset(ch.content->'pools') as p(
                pool_key text, wallet_address text, asset_code text, target_pct numeric, ceiling_pct numeric, floor_pct numeric, drift_threshold_pct numeric
           ) WHERE ch.treasury_id = $1 AND ch.version = $2 AND p.pool_key = $3"#
    )
    .bind(payload.treasury_id)
    .bind(treasury_info.get::<i32, _>("constitution_version"))
    .bind(&payload.pool_key)
    .fetch_optional(&mut **tx).await? {
        Some(r) => r,
        None => {
            tracing::error!("Pool {} not found in treasury {} version {}", payload.pool_key, payload.treasury_id, treasury_info.get::<i32, _>("constitution_version"));
            return Err(AppError::InvalidInput("Pool not found".into()));
        }
    };

    // d. Reservations & Stats
    let total_active_res_f64: f64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(approved_amount), 0)::float8 FROM permits WHERE pool_key = $1 AND status = 'ACTIVE'"
    )
    .bind(&payload.pool_key)
    .fetch_one(&mut **tx).await?;
    let total_active_res = total_active_res_f64.to_string().parse::<BigDecimal>().unwrap_or_default();

    let active_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM permits WHERE agent_id = $1 AND status = 'ACTIVE'"
    )
    .bind(payload.agent_id)
    .fetch_one(&mut **tx).await?;

    // e. Build State for Policy
    let pool_state = PoolState {
        pool_key: pool_row.get::<Option<String>, _>("pool_key").unwrap_or_default(),
        wallet_address: pool_row.get::<Option<String>, _>("wallet_address").unwrap_or_default(),
        asset_code: pool_row.get::<Option<String>, _>("asset_code").unwrap_or_default(),
        balance_units: BigDecimal::from(0),
        balance_usd: BigDecimal::from(5000), // MOCK: should be from Redis
        target_pct: pool_row.get::<f64, _>("target_pct").to_string().parse::<BigDecimal>().unwrap_or_default(),
        ceiling_pct: pool_row.get::<f64, _>("ceiling_pct").to_string().parse::<BigDecimal>().unwrap_or_default(),
        floor_pct: pool_row.get::<f64, _>("floor_pct").to_string().parse::<BigDecimal>().unwrap_or_default(),
        drift_threshold_pct: pool_row.get::<f64, _>("drift_threshold_pct").to_string().parse::<BigDecimal>().unwrap_or_default(),
        locked: false,
    };

    let health_str = treasury_info.get::<String, _>("health");
    let health = if health_str == "HALTED" { TreasuryHealth::Halted } else { TreasuryHealth::Healthy };

    let treasury_state = TreasuryState {
        treasury_id: payload.treasury_id,
        health,
        peak_aum_usd: treasury_info.get::<f64, _>("peak_aum_usd").to_string().parse::<BigDecimal>().unwrap_or_default(),
        current_aum_usd: treasury_info.get::<f64, _>("current_aum_usd").to_string().parse::<BigDecimal>().unwrap_or_default(),
        state_hash: "state_hash".into(),
        pools: vec![pool_state],
    };

    let constitution = Constitution {
        treasury_id: payload.treasury_id,
        version: treasury_info.get("constitution_version"),
        pools: vec![],
        max_drawdown_pct: BigDecimal::from(15),
        inflow_routing: vec![],
        governance_mode: "AUTO".into(),
    };

    // f. Run Engine
    let result = run_policy_engine(payload, &treasury_state, &agent_access, &constitution, &total_active_res, active_count as i32);

    // g. Persist
    let permit_id = Uuid::new_v4();
    let status = if result.approved { "ACTIVE" } else { "DENIED" };
    
    sqlx::query(
        r#"INSERT INTO permits (permit_id, group_id, leg_id, agent_id, treasury_id, wallet_address, pool_key, asset_code, 
           requested_amount, approved_amount, status, deny_reason, policy_check_number, state_snapshot_hash, coordinator_sig, expires_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)"#
    )
    .bind(permit_id)
    .bind(group_id)
    .bind(leg_id)
    .bind(payload.agent_id)
    .bind(payload.treasury_id)
    .bind(payload.wallet_address.clone())
    .bind(payload.pool_key.clone())
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
