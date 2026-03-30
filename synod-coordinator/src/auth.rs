use axum::{
    async_trait,
    extract::{FromRequestParts, State},
    http::{request::Parts, StatusCode},
    response::IntoResponse,
    Json,
    routing::post,
    Router,
};
use axum_extra::headers::{authorization::Bearer, Authorization};
use axum_extra::TypedHeader;
use bcrypt::{hash, verify, DEFAULT_COST};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, decode, Header, Validation, EncodingKey, DecodingKey};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::AppState;
use synod_shared::consts::{RATE_LIMIT_PREFIX, RATE_LIMIT_WINDOW_SECS, RATE_LIMIT_MAX_ATTEMPTS};

// -- Auth Extractor --

pub struct AuthUser {
    pub user_id: Uuid,
}

#[async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let TypedHeader(Authorization(bearer)) = 
            <TypedHeader<Authorization<Bearer>> as FromRequestParts<AppState>>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::TokenInvalid)?;

        let token_data = decode::<Claims>(
            bearer.token(),
            &DecodingKey::from_secret(state.config.auth.jwt_secret.as_bytes()),
            &Validation::default(),
        ).map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => AppError::TokenExpired,
            _ => AppError::TokenInvalid,
        })?;

        Ok(AuthUser { user_id: token_data.claims.sub })
    }
}

// -- Models --

#[derive(Debug, Deserialize)]
pub struct AuthRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user_id: Uuid,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub exp: usize,
    pub iat: usize,
}

// -- Helpers --

fn create_jwt(user_id: Uuid, secret: &str, expiry_hours: u64) -> AppResult<String> {
    let expiration = Utc::now()
        .checked_add_signed(Duration::hours(expiry_hours as i64))
        .expect("valid timestamp")
        .timestamp();

    let claims = Claims {
        sub: user_id,
        iat: Utc::now().timestamp() as usize,
        exp: expiration as usize,
    };

    let header = Header::default();
    encode(&header, &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|_| AppError::Internal(anyhow::anyhow!("JWT encoding failed")))
}

async fn check_rate_limit(ip: &str, redis: &mut redis::aio::ConnectionManager) -> AppResult<()> {
    let key = format!("{}{}", RATE_LIMIT_PREFIX, ip);
    let current_attempts: u64 = redis.get(&key).await.unwrap_or(0);

    if current_attempts >= RATE_LIMIT_MAX_ATTEMPTS {
        return Err(AppError::RateLimited);
    }
    Ok(())
}

async fn record_failed_attempt(ip: &str, redis: &mut redis::aio::ConnectionManager) {
    let key = format!("{}{}", RATE_LIMIT_PREFIX, ip);
    let mut pipe = redis::pipe();
    pipe.atomic()
        .incr(&key, 1)
        .expire(&key, RATE_LIMIT_WINDOW_SECS as i64);
    
    let _: redis::RedisResult<()> = pipe.query_async(redis).await;
}

// -- Handlers --

pub async fn register(
    State(state): State<AppState>,
    Json(payload): Json<AuthRequest>,
) -> AppResult<Json<AuthResponse>> {
    let email = payload.email.to_lowercase();
    let password_hash = hash(&payload.password, state.config.auth.bcrypt_cost)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Bcrypt Error: {}", e)))?;

    let user_id = Uuid::new_v4();
    let now = Utc::now();

    let result = sqlx::query(
        "INSERT INTO users (user_id, email, password_hash, created_at, is_active)
         VALUES ($1, $2, $3, $4, true)
         RETURNING user_id"
    )
    .bind(user_id)
    .bind(&email)
    .bind(&password_hash)
    .bind(now)
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(_) => {
            let token = create_jwt(
                user_id,
                &state.config.auth.jwt_secret,
                state.config.auth.jwt_expiry_hours,
            )?;
            Ok(Json(AuthResponse { token, user_id }))
        }
        Err(e) => {
            if let sqlx::Error::Database(db_err) = &e {
                if db_err.is_unique_violation() {
                    // Confuse attacker, don't leak email existence, just say invalid here or pretend success?
                    // Spec says: "Wrong password -> 401 INVALID_CREDENTIALS generic".
                    // For registration, usually return 400.
                }
            }
            Err(e.into())
        }
    }
}

pub async fn login(
    State(state): State<AppState>,
    Json(payload): Json<AuthRequest>,
) -> AppResult<Json<AuthResponse>> {
    let mut redis_conn = state.redis.clone();
    
    // In a real app we'd extract IP from Forwarded/X-Real-IP headers.
    // For now we use a generic string to placeholder the rate limit IP tracking.
    let client_ip = "127.0.0.1";
    check_rate_limit(client_ip, &mut redis_conn).await?;

    let email = payload.email.to_lowercase();
    let row_result = sqlx::query(
        "SELECT user_id, password_hash, is_active FROM users WHERE email = $1"
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;

    if let Some(row) = row_result {
        let is_active: bool = row.get("is_active");
        if !is_active {
            record_failed_attempt(client_ip, &mut redis_conn).await;
            return Err(AppError::InvalidCredentials);
        }

        let hash_str: String = row.get("password_hash");
        if verify(&payload.password, &hash_str).unwrap_or(false) {
            let user_id: Uuid = row.get("user_id");
            let token = create_jwt(
                user_id,
                &state.config.auth.jwt_secret,
                state.config.auth.jwt_expiry_hours,
            )?;
            
            // Update last_seen
            let _ = sqlx::query("UPDATE users SET last_seen = $1 WHERE user_id = $2")
                .bind(Utc::now())
                .bind(user_id)
                .execute(&state.db)
                .await;

            return Ok(Json(AuthResponse { token, user_id }));
        }
    }

    record_failed_attempt(client_ip, &mut redis_conn).await;
    Err(AppError::InvalidCredentials)
}

// -- Passkey Mock Models & Handlers --

#[derive(Debug, Deserialize)]
pub struct PasskeyBeginRequest {
    pub email: String,
}

#[derive(Debug, Serialize)]
pub struct PasskeyBeginResponse {
    pub challenge: String,
}

#[derive(Debug, Deserialize)]
pub struct PasskeyCompleteRegisterRequest {
    pub email: String,
    pub challenge: String,
    pub credential_id: String, // Mocked hardware key ID
}

#[derive(Debug, Deserialize)]
pub struct PasskeyCompleteLoginRequest {
    pub email: String,
    pub challenge: String,
    pub credential_id: String,
}

fn passkey_chal_key(email: &str) -> String {
    format!("passkey_chal:{}", email.to_lowercase())
}

pub async fn passkey_register_begin(
    State(state): State<AppState>,
    Json(payload): Json<PasskeyBeginRequest>,
) -> AppResult<Json<PasskeyBeginResponse>> {
    let mut redis_conn = state.redis.clone();
    let challenge = Uuid::new_v4().to_string();
    let key = passkey_chal_key(&payload.email);

    let _: () = redis_conn.set_ex(&key, &challenge, 300).await
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Redis error")))?;

    Ok(Json(PasskeyBeginResponse { challenge }))
}

pub async fn passkey_register_complete(
    State(state): State<AppState>,
    Json(payload): Json<PasskeyCompleteRegisterRequest>,
) -> AppResult<Json<AuthResponse>> {
    let mut redis_conn = state.redis.clone();
    let key = passkey_chal_key(&payload.email);

    let stored_challenge: Option<String> = redis_conn.get(&key).await.unwrap_or(None);
    if stored_challenge.is_none() || stored_challenge.as_ref().unwrap() != &payload.challenge {
        return Err(AppError::ChallengeExpired);
    }
    // Consume challenge
    let _: redis::RedisResult<()> = redis_conn.del(&key).await;

    let email = payload.email.to_lowercase();
    let user_id = Uuid::new_v4();

    // In a real app we'd decode CBOR and verify the hardware signature.
    // Here we'll just insert the user pretending the hardware credential is password_hash
    let result = sqlx::query(
        "INSERT INTO users (user_id, email, password_hash, created_at, is_active)
         VALUES ($1, $2, $3, $4, true)
         RETURNING user_id"
    )
    .bind(user_id)
    .bind(&email)
    .bind(&payload.credential_id) // Mock storing passkey credential ID
    .bind(Utc::now())
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(_) => {
            let token = create_jwt(user_id, &state.config.auth.jwt_secret, state.config.auth.jwt_expiry_hours)?;
            Ok(Json(AuthResponse { token, user_id }))
        }
        Err(e) => Err(e.into()) // handle unique constraint normally
    }
}

pub async fn passkey_login_begin(
    State(state): State<AppState>,
    Json(payload): Json<PasskeyBeginRequest>,
) -> AppResult<Json<PasskeyBeginResponse>> {
    let mut redis_conn = state.redis.clone();
    
    // Make sure user exists first
    let email = payload.email.to_lowercase();
    let user_exists: bool = sqlx::query("SELECT EXISTS(SELECT 1 FROM users WHERE email = $1)")
        .bind(&email)
        .fetch_one(&state.db)
        .await
        .map(|row| row.get(0))
        .unwrap_or(false);

    if !user_exists {
        return Err(AppError::InvalidCredentials);
    }

    let challenge = Uuid::new_v4().to_string();
    let key = passkey_chal_key(&email);
    let _: () = redis_conn.set_ex(&key, &challenge, 300).await
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Redis error")))?;

    Ok(Json(PasskeyBeginResponse { challenge }))
}

pub async fn passkey_login_complete(
    State(state): State<AppState>,
    Json(payload): Json<PasskeyCompleteLoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    let mut redis_conn = state.redis.clone();
    let key = passkey_chal_key(&payload.email);

    let stored_challenge: Option<String> = redis_conn.get(&key).await.unwrap_or(None);
    if stored_challenge.is_none() || stored_challenge.as_ref().unwrap() != &payload.challenge {
        return Err(AppError::ChallengeExpired);
    }
    // Consume challenge
    let _: redis::RedisResult<()> = redis_conn.del(&key).await;

    let email = payload.email.to_lowercase();
    let row_result = sqlx::query(
        "SELECT user_id, password_hash, is_active FROM users WHERE email = $1"
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;

    if let Some(row) = row_result {
        let is_active: bool = row.get("is_active");
        if !is_active {
            return Err(AppError::InvalidCredentials);
        }

        let stored_cred: String = row.get("password_hash");
        if stored_cred == payload.credential_id {
            let user_id: Uuid = row.get("user_id");
            let token = create_jwt(user_id, &state.config.auth.jwt_secret, state.config.auth.jwt_expiry_hours)?;
            
            let _ = sqlx::query("UPDATE users SET last_seen = $1 WHERE user_id = $2")
                .bind(Utc::now())
                .bind(user_id)
                .execute(&state.db)
                .await;

            return Ok(Json(AuthResponse { token, user_id }));
        }
    }

    Err(AppError::InvalidCredentials)
}

// -- Router Setup --

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/passkey/register/begin", post(passkey_register_begin))
        .route("/passkey/register/complete", post(passkey_register_complete))
        .route("/passkey/login/begin", post(passkey_login_begin))
        .route("/passkey/login/complete", post(passkey_login_complete))
}
