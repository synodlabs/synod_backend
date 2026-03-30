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
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
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
            AppError::PermitNotFound => (StatusCode::NOT_FOUND, "PERMIT_NOT_FOUND"),
            AppError::PermitExpired => (StatusCode::GONE, "PERMIT_EXPIRED"),
            AppError::CosignFailed(_) => (StatusCode::BAD_REQUEST, "COSIGN_FAILED"),
            AppError::ConcurrentLimitReached => (StatusCode::TOO_MANY_REQUESTS, "CONCURRENT_LIMIT"),
            AppError::WalletNotFound => (StatusCode::NOT_FOUND, "WALLET_NOT_FOUND"),
            AppError::OwnershipVerificationFailed => (StatusCode::FORBIDDEN, "OWNERSHIP_FAILED"),
            AppError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "DATABASE_ERROR"),
            AppError::Redis(_) => (StatusCode::INTERNAL_SERVER_ERROR, "REDIS_ERROR"),
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
