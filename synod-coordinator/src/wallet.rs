use axum::extract::State;
use axum::{routing::post, Json, Router};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::error::{AppError, AppResult};
use crate::{AppState, stellar};
use crate::auth::AuthUser;

#[derive(Debug, Deserialize)]
pub struct NonceRequest {
    pub wallet_address: String,
}

#[derive(Debug, Serialize)]
pub struct NonceResponse {
    pub nonce: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyOwnershipRequest {
    pub wallet_address: String,
    pub signature: String,
    pub nonce: String,
}

#[derive(Debug, Serialize)]
pub struct VerifyOwnershipResponse {
    pub verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct WalletConnectRequest {
    pub wallet_address: String,
    pub wc_session_topic: String,
    pub wc_session_expiry: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct ConnectResponse {
    pub success: bool,
}

pub async fn get_nonce(
    State(state): State<AppState>,
    Json(payload): Json<NonceRequest>,
) -> AppResult<Json<NonceResponse>> {
    let mut redis_conn = state.redis.clone();
    let nonce = Uuid::new_v4().to_string();
    let key = format!("nonce:{}", payload.wallet_address);

    let _: () = redis_conn.set_ex(&key, &nonce, 600).await
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Redis error")))?;

    Ok(Json(NonceResponse { nonce }))
}

pub async fn verify_ownership(
    State(state): State<AppState>,
    _auth: AuthUser,
    Json(payload): Json<VerifyOwnershipRequest>,
) -> AppResult<Json<VerifyOwnershipResponse>> {
    let mut redis_conn = state.redis.clone();
    let key = format!("nonce:{}", payload.wallet_address);

    let stored_nonce: Option<String> = redis_conn.get(&key).await.unwrap_or(None);
    if stored_nonce.is_none() || stored_nonce.unwrap() != payload.nonce {
        return Err(AppError::ChallengeExpired);
    }
    
    // Consume nonce
    let _: redis::RedisResult<()> = redis_conn.del(&key).await;

    // Verify Ed25519 signature
    stellar::verify_stellar_signature(&payload.wallet_address, payload.nonce.as_bytes(), &payload.signature)?;
    
    Ok(Json(VerifyOwnershipResponse { verified: true }))
}

pub async fn connect_wallet(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<WalletConnectRequest>,
) -> AppResult<Json<ConnectResponse>> {
    sqlx::query(
        "UPDATE wallet_connections 
         SET wc_session_topic = $1, wc_session_expiry = $2, status = 'ACTIVE'
         WHERE user_id = $3 AND wallet_address = $4"
    )
    .bind(&payload.wc_session_topic)
    .bind(payload.wc_session_expiry)
    .bind(auth.user_id)
    .bind(&payload.wallet_address)
    .execute(&state.db)
    .await?;

    Ok(Json(ConnectResponse { success: true }))
}

pub async fn heartbeat_wallet(
    _auth: AuthUser,
) -> AppResult<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "status": "alive" })))
}

pub async fn disconnect_wallet(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<NonceRequest>, // Reusing NonceRequest for wallet_address
) -> AppResult<Json<ConnectResponse>> {
    sqlx::query(
        "UPDATE wallet_connections 
         SET status = 'DISCONNECTED', disconnected_at = $1
         WHERE user_id = $2 AND wallet_address = $3"
    )
    .bind(chrono::Utc::now())
    .bind(auth.user_id)
    .bind(&payload.wallet_address)
    .execute(&state.db)
    .await?;

    Ok(Json(ConnectResponse { success: true }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/nonce", post(get_nonce))
        .route("/verify-ownership", post(verify_ownership))
        .route("/connect", post(connect_wallet))
        .route("/heartbeat", post(heartbeat_wallet))
        .route("/disconnect", post(disconnect_wallet))
}
