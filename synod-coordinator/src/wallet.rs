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
    auth: AuthUser,
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
    stellar::verify_stellar_signature(
        &payload.wallet_address, 
        payload.nonce.as_bytes(), 
        &payload.signature,
        &state.config.stellar.network_passphrase,
    )?;
    
    // Update status to ACTIVE for any treasury using this wallet
    sqlx::query(
        "UPDATE treasury_wallets SET status = 'ACTIVE' WHERE wallet_address = $1"
    )
    .bind(&payload.wallet_address)
    .execute(&state.db)
    .await?;

    // Transition PENDING_WALLET treasuries to HEALTHY if they now have an active wallet
    sqlx::query(
        r#"
        UPDATE treasuries 
        SET health = 'HEALTHY', updated_at = NOW() 
        WHERE health = 'PENDING_WALLET' 
        AND treasury_id IN (SELECT treasury_id FROM treasury_wallets WHERE wallet_address = $1 AND status = 'ACTIVE')
        "#
    )
    .bind(&payload.wallet_address)
    .execute(&state.db)
    .await?;

    // Also update/insert a verified connection for this user
    sqlx::query(
        r#"
        INSERT INTO wallet_connections (user_id, wallet_address, status, verified_at, wc_session_topic, wc_session_expiry, ownership_sig, ownership_sig_hash, network)
        VALUES ($1, $2, 'ACTIVE', NOW(), '', NOW(), $3, '', $4)
        ON CONFLICT (wallet_address) DO UPDATE SET status = 'ACTIVE', verified_at = NOW(), ownership_sig = $3, network = $4
        "#
    )
    .bind(auth.user_id)
    .bind(&payload.wallet_address)
    .bind(&payload.signature)
    .bind(&state.config.stellar.network)
    .execute(&state.db)
    .await?;

    // Spawn Horizon watcher for this wallet
    let treasury_row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT treasury_id FROM treasury_wallets WHERE wallet_address = $1 AND status = 'ACTIVE' LIMIT 1"
    )
    .bind(&payload.wallet_address)
    .fetch_optional(&state.db)
    .await?;

    if let Some((treasury_id,)) = treasury_row {
        let wallet_addr = payload.wallet_address.clone();
        let state_clone = state.clone();
        let handle = tokio::spawn(async move {
            let mut watcher = crate::horizon::HorizonWatcher::new(
                wallet_addr.clone(),
                treasury_id,
                state_clone,
            );
            watcher.run().await;
        });

        let mut handles = state.watcher_handles.lock().await;
        // If a watcher was already running for this wallet, abort it first
        if let Some(old) = handles.insert(payload.wallet_address.clone(), handle) {
            old.abort();
        }
    }

    Ok(Json(VerifyOwnershipResponse { verified: true }))
}

pub async fn check_verified(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<NonceRequest>,
) -> AppResult<Json<VerifyOwnershipResponse>> {
    let exists = sqlx::query!(
        "SELECT 1 as id FROM wallet_connections WHERE user_id = $1 AND wallet_address = $2 AND status = 'ACTIVE'",
        auth.user_id,
        payload.wallet_address
    )
    .fetch_optional(&state.db)
    .await?
    .is_some();

    Ok(Json(VerifyOwnershipResponse { verified: exists }))
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

    // Abort the Horizon watcher for this wallet
    {
        let mut handles = state.watcher_handles.lock().await;
        if let Some(handle) = handles.remove(&payload.wallet_address) {
            handle.abort();
            tracing::info!(wallet = %payload.wallet_address, "Horizon watcher stopped");
        }
    }

    Ok(Json(ConnectResponse { success: true }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/nonce", post(get_nonce))
        .route("/verify-ownership", post(verify_ownership))
        .route("/check-verified", post(check_verified))
        .route("/connect", post(connect_wallet))
        .route("/heartbeat", post(heartbeat_wallet))
        .route("/disconnect", post(disconnect_wallet))
}
