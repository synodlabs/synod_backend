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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_treasury))
        .route("/:id/wallets", post(register_wallet))
}
