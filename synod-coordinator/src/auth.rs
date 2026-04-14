use axum::{
    async_trait,
    extract::{FromRequestParts, State},
    http::HeaderMap,
    http::request::Parts,
    response::IntoResponse,
    Json,
    routing::{get, post},
    Router,
};
use bcrypt::{hash, verify};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, decode, Header, Validation, EncodingKey, DecodingKey};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::AppState;
use crate::stellar;
use synod_shared::consts::{RATE_LIMIT_PREFIX, RATE_LIMIT_WINDOW_SECS, RATE_LIMIT_MAX_ATTEMPTS};

// -- Auth Extractor --

pub struct AuthUser {
    pub user_id: Uuid,
}

pub struct AgentAuth {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub agent_pubkey: String,
    pub session_token: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentSession {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub agent_pubkey: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SignedRequestAuth {
    pub agent_pubkey: String,
    pub request_id: String,
    pub timestamp: i64,
    pub signature: String,
}

#[async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        // 1. Try Authorization header
        let token = if let Some(auth_header) = parts.headers.get(axum::http::header::AUTHORIZATION) {
            let auth_str = auth_header.to_str().map_err(|_| AppError::TokenInvalid)?;
            if !auth_str.starts_with("Bearer ") {
                return Err(AppError::TokenInvalid);
            }
            Some(auth_str[7..].to_string())
        } else {
            None
        };

        // 2. Try httpOnly cookie
        let token = token.or_else(|| {
            parts.headers.get(axum::http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .and_then(|cookies| {
                    cookies.split(';')
                        .find_map(|c| {
                            let c = c.trim();
                            c.strip_prefix("synod_session=").map(|v| v.to_string())
                        })
                })
        });

        // 3. Try query parameter (fallback for WebSockets)
        let token = token.or_else(|| {
            let query = parts.uri.query().unwrap_or("");
            query.split('&')
                .find(|part| part.starts_with("auth="))
                .map(|part| part[5..].to_string())
        });

        let token = token.ok_or(AppError::TokenInvalid)?;

        let token_data = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(state.config.auth.jwt_secret.as_bytes()),
            &Validation::default(),
        ).map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => AppError::TokenExpired,
            _ => AppError::TokenInvalid,
        })?;

        let user_id = token_data.claims.sub;

        // 4. Verify user exists in DB to prevent stale tokens/FK violations
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE user_id = $1)")
            .bind(user_id)
            .fetch_one(&state.db)
            .await
            .map_err(|e| AppError::Database(e))?;

        if !exists {
            return Err(AppError::TokenInvalid);
        }

        Ok(AuthUser { user_id })
    }
}

#[async_trait]
impl FromRequestParts<AppState> for AgentAuth {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = extract_bearer_token(parts)?;
        let session = load_agent_session(&token, state).await?;

        let status: String = sqlx::query_scalar(
            "SELECT status FROM agent_slots WHERE agent_id = $1 AND treasury_id = $2"
        )
        .bind(session.agent_id)
        .bind(session.treasury_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::InvalidAgentSession)?;

        match status.as_str() {
            "SUSPENDED" => return Err(AppError::AgentSuspended),
            "REVOKED" => return Err(AppError::AgentRevoked),
            _ => {}
        }

        Ok(AgentAuth {
            agent_id: session.agent_id,
            treasury_id: session.treasury_id,
            agent_pubkey: session.agent_pubkey,
            session_token: token,
        })
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

fn make_session_cookie(token: &str, max_age_hours: u64) -> String {
    format!(
        "synod_session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}",
        token,
        max_age_hours * 3600
    )
}

fn extract_bearer_token(parts: &Parts) -> AppResult<String> {
    let auth_header = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .ok_or(AppError::InvalidAgentSession)?;

    let auth_str = auth_header.to_str().map_err(|_| AppError::InvalidAgentSession)?;
    if !auth_str.starts_with("Bearer ") {
        return Err(AppError::InvalidAgentSession);
    }

    Ok(auth_str[7..].to_string())
}

pub fn agent_session_key(token: &str) -> String {
    format!("agent:session:{}", token)
}

pub fn agent_request_replay_key(agent_id: Uuid, request_id: &str) -> String {
    format!("agent:req:{}:{}", agent_id, request_id)
}

pub async fn store_agent_session(
    state: &AppState,
    token: &str,
    session: &AgentSession,
    ttl_seconds: u64,
) -> AppResult<()> {
    let mut redis_conn = state.redis.clone();
    let value = serde_json::to_string(session)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Session serialization failed: {}", e)))?;

    let _: () = redis_conn
        .set_ex(agent_session_key(token), value, ttl_seconds)
        .await
        .map_err(AppError::Redis)?;

    Ok(())
}

pub async fn load_agent_session(token: &str, state: &AppState) -> AppResult<AgentSession> {
    let mut redis_conn = state.redis.clone();
    let stored: Option<String> = redis_conn
        .get(agent_session_key(token))
        .await
        .map_err(AppError::Redis)?;

    let stored = stored.ok_or(AppError::InvalidAgentSession)?;
    let session: AgentSession = serde_json::from_str(&stored)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Session decode failed: {}", e)))?;

    if session.expires_at <= Utc::now().timestamp() {
        let _: redis::RedisResult<()> = redis_conn.del(agent_session_key(token)).await;
        return Err(AppError::InvalidAgentSession);
    }

    Ok(session)
}

pub async fn verify_signed_request<T: Serialize>(
    state: &AppState,
    agent_auth: &AgentAuth,
    op_name: &str,
    payload: &T,
    auth: &SignedRequestAuth,
) -> AppResult<()> {
    if auth.agent_pubkey != agent_auth.agent_pubkey {
        return Err(AppError::RequestSignatureInvalid);
    }

    let now = Utc::now().timestamp();
    if (now - auth.timestamp).abs() > 300 {
        return Err(AppError::ChallengeExpired);
    }

    let replay_key = agent_request_replay_key(agent_auth.agent_id, &auth.request_id);
    let mut redis_conn = state.redis.clone();
    let already_seen: bool = redis_conn.exists(&replay_key).await.map_err(AppError::Redis)?;
    if already_seen {
        return Err(AppError::RequestReplay);
    }

    let payload_json = serde_json::to_string(payload)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Request serialization failed: {}", e)))?;
    let message = format!(
        "synod-request:{}:{}:{}:{}:{}",
        op_name,
        agent_auth.agent_id,
        auth.request_id,
        auth.timestamp,
        payload_json
    );

    stellar::verify_stellar_signature(
        &auth.agent_pubkey,
        message.as_bytes(),
        &auth.signature,
        &state.config.stellar.network_passphrase,
    )
    .map_err(|_| AppError::RequestSignatureInvalid)?;

    let _: () = redis_conn.set_ex(replay_key, "1", 600).await.map_err(AppError::Redis)?;
    Ok(())
}

async fn check_rate_limit(ip: &str, redis: &mut redis::aio::ConnectionManager) -> AppResult<()> {
    let key = format!("{}{}", RATE_LIMIT_PREFIX, ip);
    let current_attempts: u64 = redis.get(&key).await.unwrap_or(0);

    if current_attempts >= RATE_LIMIT_MAX_ATTEMPTS {
        return Err(AppError::RateLimited);
    }
    Ok(())
}

fn extract_rate_limit_subject(headers: &HeaderMap, email: &str) -> String {
    let forwarded_ip = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let real_ip = headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let client_id = forwarded_ip.or(real_ip).unwrap_or("local");
    format!("{}:{}", client_id, email.to_lowercase())
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
) -> AppResult<impl IntoResponse> {
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
            let cookie = make_session_cookie(&token, state.config.auth.jwt_expiry_hours);
            Ok((
                [(axum::http::header::SET_COOKIE, cookie)],
                Json(AuthResponse { token, user_id }),
            ))
        }
        Err(e) => {
            if let sqlx::Error::Database(db_err) = &e {
                if db_err.is_unique_violation() {
                    // Don't leak email existence
                }
            }
            Err(e.into())
        }
    }
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AuthRequest>,
) -> AppResult<impl IntoResponse> {
    let mut redis_conn = state.redis.clone();

    let email = payload.email.to_lowercase();
    let rate_limit_subject = extract_rate_limit_subject(&headers, &email);
    check_rate_limit(&rate_limit_subject, &mut redis_conn).await?;
    let row_result = sqlx::query(
        "SELECT user_id, password_hash, is_active FROM users WHERE email = $1"
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;

    if let Some(row) = row_result {
        let is_active: bool = row.get("is_active");
        if !is_active {
            record_failed_attempt(&rate_limit_subject, &mut redis_conn).await;
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

            let cookie = make_session_cookie(&token, state.config.auth.jwt_expiry_hours);
            return Ok((
                [(axum::http::header::SET_COOKIE, cookie)],
                Json(AuthResponse { token, user_id }),
            ));
        }
    }

    record_failed_attempt(&rate_limit_subject, &mut redis_conn).await;
    Err(AppError::InvalidCredentials)
}

// -- /me and /logout --

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub user_id: Uuid,
    pub authenticated: bool,
}

pub async fn me(
    auth: AuthUser,
) -> AppResult<Json<MeResponse>> {
    Ok(Json(MeResponse {
        user_id: auth.user_id,
        authenticated: true,
    }))
}

pub async fn logout() -> impl IntoResponse {
    let cookie = "synod_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0";
    (
        [(axum::http::header::SET_COOKIE, cookie.to_string())],
        Json(serde_json::json!({ "status": "logged_out" })),
    )
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
    pub credential_id: String,
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
    let _: redis::RedisResult<()> = redis_conn.del(&key).await;

    let email = payload.email.to_lowercase();
    let user_id = Uuid::new_v4();

    let result = sqlx::query(
        "INSERT INTO users (user_id, email, password_hash, created_at, is_active)
         VALUES ($1, $2, $3, $4, true)
         RETURNING user_id"
    )
    .bind(user_id)
    .bind(&email)
    .bind(&payload.credential_id)
    .bind(Utc::now())
    .fetch_one(&state.db)
    .await;

    match result {
        Ok(_) => {
            let token = create_jwt(user_id, &state.config.auth.jwt_secret, state.config.auth.jwt_expiry_hours)?;
            Ok(Json(AuthResponse { token, user_id }))
        }
        Err(e) => Err(e.into())
    }
}

pub async fn passkey_login_begin(
    State(state): State<AppState>,
    Json(payload): Json<PasskeyBeginRequest>,
) -> AppResult<Json<PasskeyBeginResponse>> {
    let mut redis_conn = state.redis.clone();
    
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
        .route("/me", get(me))
        .route("/logout", post(logout))
        .route("/passkey/register/begin", post(passkey_register_begin))
        .route("/passkey/register/complete", post(passkey_register_complete))
        .route("/passkey/login/begin", post(passkey_login_begin))
        .route("/passkey/login/complete", post(passkey_login_complete))
}
