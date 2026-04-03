use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    // Auth errors
    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Token expired")]
    TokenExpired,

    #[error("Token invalid")]
    TokenInvalid,

    #[error("Challenge expired")]
    ChallengeExpired,

    #[error("Rate limited")]
    RateLimited,

    // Treasury errors
    #[error("Treasury not found")]
    TreasuryNotFound,

    #[error("Treasury halted")]
    TreasuryHalted,

    #[error("Allocation sum invalid")]
    AllocationSumInvalid,

    #[error("Pool bounds conflict")]
    PoolBoundsConflict,

    // Agent errors
    #[error("Agent not found")]
    AgentNotFound,

    #[error("Agent suspended")]
    AgentSuspended,

    #[error("Agent revoked")]
    AgentRevoked,

    #[error("Pubkey conflict: agent slot already has a different registered keypair")]
    PubkeyConflict,

    #[error("Signer authorization declined by wallet owner")]
    SignerAuthDeclined,

    #[error("Wallet session unavailable — user must reconnect wallet via dashboard")]
    WalletSessionUnavailable,

    #[error("Signer authorization timed out")]
    SignerAuthTimeout,

    #[error("SetOptions submission failed: {0}")]
    SetOptionsSubmissionFailed(String),

    // Permit errors
    #[error("Permit not found")]
    PermitNotFound,

    #[error("Permit expired")]
    PermitExpired,

    #[error("Co-sign verification failed: {0}")]
    CosignFailed(String),

    #[error("Concurrent limit reached")]
    ConcurrentLimitReached,

    // Wallet errors
    #[error("Wallet not found")]
    WalletNotFound,

    #[error("Ownership verification failed")]
    OwnershipVerificationFailed,

    // Infrastructure errors
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("Not Found: {0}")]
    NotFound(String),
    #[error("Invalid Input: {0}")]
    InvalidInput(String),

    #[error("Invalid API key")]
    InvalidApiKey,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if matches!(self, AppError::Internal(_)) {
            tracing::error!("Internal Server Error: {:?}", self);
        } else {
            tracing::warn!("AppError: {:?}", self);
        }

        let (status, error_code) = match &self {
            AppError::InvalidCredentials => (StatusCode::UNAUTHORIZED, "INVALID_CREDENTIALS"),
            AppError::TokenExpired => (StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED"),
            AppError::TokenInvalid => (StatusCode::UNAUTHORIZED, "TOKEN_INVALID"),
            AppError::ChallengeExpired => (StatusCode::UNAUTHORIZED, "CHALLENGE_EXPIRED"),
            AppError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED"),
            AppError::TreasuryNotFound => (StatusCode::NOT_FOUND, "TREASURY_NOT_FOUND"),
            AppError::TreasuryHalted => (StatusCode::FORBIDDEN, "TREASURY_HALTED"),
            AppError::AllocationSumInvalid => (StatusCode::UNPROCESSABLE_ENTITY, "ALLOCATION_SUM_INVALID"),
            AppError::PoolBoundsConflict => (StatusCode::UNPROCESSABLE_ENTITY, "POOL_BOUNDS_CONFLICT"),
            AppError::AgentNotFound => (StatusCode::NOT_FOUND, "AGENT_NOT_FOUND"),
            AppError::AgentSuspended => (StatusCode::FORBIDDEN, "AGENT_SUSPENDED"),
            AppError::AgentRevoked => (StatusCode::FORBIDDEN, "AGENT_REVOKED"),
            AppError::PubkeyConflict => (StatusCode::CONFLICT, "PUBKEY_CONFLICT"),
            AppError::SignerAuthDeclined => (StatusCode::FORBIDDEN, "SIGNER_AUTHORIZATION_DECLINED"),
            AppError::WalletSessionUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "WALLET_SESSION_UNAVAILABLE"),
            AppError::SignerAuthTimeout => (StatusCode::REQUEST_TIMEOUT, "SIGNER_AUTHORIZATION_TIMEOUT"),
            AppError::SetOptionsSubmissionFailed(_) => (StatusCode::INTERNAL_SERVER_ERROR, "SETOPTIONS_SUBMISSION_FAILED"),
            AppError::PermitNotFound => (StatusCode::NOT_FOUND, "PERMIT_NOT_FOUND"),
            AppError::PermitExpired => (StatusCode::GONE, "PERMIT_EXPIRED"),
            AppError::CosignFailed(_) => (StatusCode::BAD_REQUEST, "COSIGN_FAILED"),
            AppError::ConcurrentLimitReached => (StatusCode::TOO_MANY_REQUESTS, "CONCURRENT_LIMIT"),
            AppError::WalletNotFound => (StatusCode::NOT_FOUND, "WALLET_NOT_FOUND"),
            AppError::OwnershipVerificationFailed => (StatusCode::FORBIDDEN, "OWNERSHIP_FAILED"),
            AppError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "DATABASE_ERROR"),
            AppError::Redis(_) => (StatusCode::INTERNAL_SERVER_ERROR, "REDIS_ERROR"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            AppError::InvalidInput(_) => (StatusCode::BAD_REQUEST, "INVALID_INPUT"),
            AppError::InvalidApiKey => (StatusCode::UNAUTHORIZED, "INVALID_API_KEY"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        };

        let body = serde_json::json!({
            "error": error_code,
            "message": self.to_string(),
        });

        (status, Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
