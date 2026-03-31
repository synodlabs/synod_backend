use axum::extract::{Path, State};
use axum::{routing::post, Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::error::{AppError, AppResult};
use crate::AppState;
use crate::auth::AuthUser;
use sqlx::Row;

#[derive(Debug, Deserialize)]
pub struct CreateTreasuryRequest {
    pub name: String,
    pub network: String,
}

#[derive(Debug, Serialize)]
pub struct TreasuryResponse {
    pub treasury_id: Uuid,
    pub name: String,
    pub health: String,
}

#[derive(Debug, Deserialize)]
pub struct RegisterWalletRequest {
    pub wallet_address: String,
    pub label: Option<String>,
}

pub async fn create_treasury(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateTreasuryRequest>,
) -> AppResult<Json<TreasuryResponse>> {
    let treasury_id = Uuid::new_v4();
    
    sqlx::query(
        "INSERT INTO treasuries (treasury_id, owner_user_id, name, network, health, created_at, updated_at)
         VALUES ($1, $2, $3, $4, 'PENDING_WALLET', $5, $5)"
    )
    .bind(treasury_id)
    .bind(auth.user_id)
    .bind(&payload.name)
    .bind(&payload.network)
    .bind(Utc::now())
    .execute(&state.db)
    .await?;

    Ok(Json(TreasuryResponse {
        treasury_id,
        name: payload.name,
        health: "PENDING_WALLET".to_string(),
    }))
}

use axum::http::StatusCode;

pub async fn register_wallet(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<RegisterWalletRequest>,
) -> AppResult<StatusCode> {

    // Verify treasury ownership
    let is_owner: bool = sqlx::query("SELECT EXISTS(SELECT 1 FROM treasuries WHERE treasury_id = $1 AND owner_user_id = $2)")
        .bind(treasury_id)
        .bind(auth.user_id)
        .fetch_one(&state.db)
        .await
        .map(|row| row.get(0))
        .unwrap_or(false);

    if !is_owner {
        return Err(AppError::TreasuryNotFound);
    }

    sqlx::query(
        "INSERT INTO treasury_wallets (wallet_id, treasury_id, wallet_address, label, multisig_active, status, added_at)
         VALUES ($1, $2, $3, $4, false, 'PENDING', $5)"
    )
    .bind(Uuid::new_v4())
    .bind(treasury_id)
    .bind(&payload.wallet_address)
    .bind(payload.label)
    .bind(Utc::now())
    .execute(&state.db)
    .await?;

    Ok(StatusCode::CREATED)
}

pub async fn apply_halt(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    treasury_id: Uuid,
) -> AppResult<()> {
    // 1. Acquire Mutex (Postgres Advisory Lock) to prevent race conditions during halt
    let lock_key = (treasury_id.as_u128() & 0xFFFFFFFF) as i64;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(lock_key)
        .execute(&mut **tx).await?;

    // 2. Set Treasury Health
    sqlx::query("UPDATE treasuries SET health = 'HALTED', updated_at = NOW() WHERE treasury_id = $1")
        .bind(treasury_id)
        .execute(&mut **tx).await?;

    // 3. Lock Pools in current constitution
    // In a real system, we'd create a new constitution version with locked: true for all pools.
    // For now, we'll just update the current JSON if simple, or assume the policy engine checks 'health = HALTED'.
    // The policy engine already checks check_treasury_halted(treasury_state).

    // 4. Revoke All Active Permits
    sqlx::query("UPDATE permits SET status = 'REVOKED' WHERE treasury_id = $1 AND status = 'ACTIVE'")
        .bind(treasury_id)
        .execute(&mut **tx).await?;

    Ok(())
}

pub async fn resume_treasury(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    // 1. Verify Ownership
    let is_owner: bool = sqlx::query("SELECT EXISTS(SELECT 1 FROM treasuries WHERE treasury_id = $1 AND owner_user_id = $2)")
        .bind(treasury_id)
        .bind(auth.user_id)
        .fetch_one(&state.db)
        .await?
        .get(0);

    if !is_owner {
        return Err(AppError::TreasuryNotFound);
    }

    let mut tx = state.db.begin().await?;

    // 2. Reset Drawdown & Unlock
    sqlx::query(
        "UPDATE treasuries SET health = 'HEALTHY', peak_aum_usd = current_aum_usd, updated_at = NOW() WHERE treasury_id = $1"
    )
    .bind(treasury_id)
    .execute(&mut *tx).await?;

    tx.commit().await?;

    Ok(StatusCode::OK)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_treasury))
        .route("/:id/wallets", post(register_wallet))
        .route("/:id/resume", post(resume_treasury))
}
