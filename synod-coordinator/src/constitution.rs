use axum::extract::{Path, State};
use axum::{routing::{get, post}, Json, Router};
// use bigdecimal::{BigDecimal, Zero};
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use tracing::info;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

// ── Models ──

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PoolConfig {
    pub pool_key: String,
    pub asset_code: String,
    pub target_pct: f64,
    pub floor_pct: f64,
    pub ceiling_pct: f64,
    #[serde(default, alias = "drift_bounds_pct")]
    pub drift_threshold_pct: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConstitutionContent {
    pub pools: Vec<PoolConfig>,
    pub memo: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConstitutionHistory {
    pub version: i32,
    pub treasury_id: Uuid,
    pub state_hash: String,
    pub content: ConstitutionContent,
    pub executed_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateConstitutionRequest {
    pub content: ConstitutionContent,
}

#[derive(Debug, Serialize)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
}

// ── Validation Logic ──

pub fn validate_constitution(content: &ConstitutionContent) -> ValidationResult {
    let mut errors = Vec::new();
    let mut total_target = 0.0;

    if content.pools.is_empty() {
        errors.push("Constitution must have at least one pool".to_string());
    }

    for pool in &content.pools {
        total_target += pool.target_pct;

        if pool.target_pct < pool.floor_pct || pool.target_pct > pool.ceiling_pct {
            errors.push(format!(
                "Pool {}: target ({}) must be between floor ({}) and ceiling ({})",
                pool.pool_key, pool.target_pct, pool.floor_pct, pool.ceiling_pct
            ));
        }

        if pool.drift_threshold_pct > (pool.ceiling_pct - pool.target_pct) {
            errors.push(format!(
                "Pool {}: drift bounds ({}) exceed target-to-ceiling margin ({})",
                pool.pool_key, pool.drift_threshold_pct, pool.ceiling_pct - pool.target_pct
            ));
        }

        if pool.drift_threshold_pct > (pool.target_pct - pool.floor_pct) {
            errors.push(format!(
                "Pool {}: drift bounds ({}) exceed target-to-floor margin ({})",
                pool.pool_key, pool.drift_threshold_pct, pool.target_pct - pool.floor_pct
            ));
        }
    }

    // Floating point math: check if total is extremely close to 100.0
    if (total_target - 100.0).abs() > 0.0001 {
        errors.push(format!("Total target_pct must equal 100.0 (got {})", total_target));
    }

    ValidationResult {
        valid: errors.is_empty(),
        errors,
    }
}

pub fn generate_state_hash(content: &ConstitutionContent) -> AppResult<String> {
    // Canonical JSON representation
    let json_bytes = serde_json::to_vec(content).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("Failed to serialize constitution for hashing: {}", e))
    })?;
    
    let hash = Sha256::digest(&json_bytes);
    Ok(hex::encode(hash))
}

// ── Endpoints ──

pub async fn get_current_constitution(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
) -> AppResult<Json<ConstitutionHistory>> {
    let raw: Option<(i32, String, serde_json::Value, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT version, state_hash, content, executed_at 
        FROM constitution_history 
        WHERE treasury_id = $1 
        ORDER BY version DESC LIMIT 1
        "#
    )
    .bind(treasury_id)
    .fetch_optional(&state.db)
    .await?;

    match raw {
        Some((version, state_hash, content_json, executed_at)) => {
            let content: ConstitutionContent = serde_json::from_value(content_json).map_err(|e| {
                AppError::Internal(anyhow::anyhow!("Failed to parse JSON content: {}", e))
            })?;
            Ok(Json(ConstitutionHistory {
                version,
                treasury_id,
                state_hash,
                content,
                executed_at,
            }))
        }
        None => Err(AppError::NotFound("Constitution not found".to_string())),
    }
}

pub async fn create_or_update_constitution(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<CreateConstitutionRequest>,
) -> AppResult<(StatusCode, Json<ConstitutionHistory>)> {
    info!(treasury = %treasury_id, "Hit create_or_update_constitution");
    // 1. Validate rules
    let validation = validate_constitution(&payload.content);
    if !validation.valid {
        return Err(AppError::InvalidInput(validation.errors.join(", ")));
    }

    // 2. Generate Hash
    let hash = generate_state_hash(&payload.content)?;

    // 3. Determine next version
    let max_v: i32 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM constitution_history WHERE treasury_id = $1")
        .bind(treasury_id)
        .fetch_one(&state.db)
        .await?;
    let next_version = max_v + 1;

    // 4. Insert into database
    let content_json = serde_json::to_value(&payload.content).unwrap();
    
    // In a real multi-sig governance scenario, this endpoint would either be restricted to admins
    // or not exist directly, and instead be applied by the Proposal Executor.
    // We implement it here to satisfy CRUD requirements before Proposal implementation.
    
    let mut tx = state.db.begin().await?;

    sqlx::query(
        r#"
        INSERT INTO constitution_history (treasury_id, version, state_hash, content) 
        VALUES ($1, $2, $3, $4)
        "#
    )
    .bind(treasury_id)
    .bind(next_version)
    .bind(&hash)
    .bind(&content_json)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE treasuries SET constitution_version = $1, updated_at = $2 WHERE treasury_id = $3")
        .bind(next_version)
        .bind(Utc::now())
        .bind(treasury_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    info!(treasury = %treasury_id, version = %next_version, "Constitution updated");

    // Cache in Redis
    use redis::AsyncCommands;
    let mut redis = state.redis.clone();
    let cache_key = format!("constitution:{}", treasury_id);
    let _: () = redis.set(&cache_key, serde_json::to_string(&payload.content).unwrap()).await.unwrap_or(());

    // Re-fetch to return
    let res = get_current_constitution(State(state.clone()), _auth, Path(treasury_id)).await?;
    
    let _ = state.tx_events.send(crate::TreasuryEvent::ConstitutionUpdate {
        treasury_id,
        version: next_version,
    });

    Ok((StatusCode::CREATED, res))
}

pub async fn rollback_constitution(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((treasury_id, target_version)): Path<(Uuid, i32)>,
) -> AppResult<(StatusCode, Json<ConstitutionHistory>)> {
    // 1. Fetch old version
    let old_raw: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT content FROM constitution_history WHERE treasury_id = $1 AND version = $2"
    )
    .bind(treasury_id)
    .bind(target_version)
    .fetch_optional(&state.db)
    .await?;

    let content_json = old_raw.ok_or_else(|| AppError::NotFound("Target version not found".to_string()))?.0;
    
    let content: ConstitutionContent = serde_json::from_value(content_json).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("Old constitution malformed: {}", e))
    })?;

    // 2. Wrap into create request and re-apply
    create_or_update_constitution(
        State(state),
        auth,
        Path(treasury_id),
        Json(CreateConstitutionRequest { content })
    ).await
}

pub async fn get_constitution_history(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
) -> AppResult<Json<Vec<ConstitutionHistory>>> {
    let rows: Vec<(i32, String, serde_json::Value, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT version, state_hash, content, executed_at 
        FROM constitution_history 
        WHERE treasury_id = $1 
        ORDER BY version DESC
        "#
    )
    .bind(treasury_id)
    .fetch_all(&state.db)
    .await?;

    let mut history = Vec::new();
    for (version, state_hash, content_json, executed_at) in rows {
        let content: ConstitutionContent = serde_json::from_value(content_json).map_err(|e| {
            AppError::Internal(anyhow::anyhow!("Failed to parse JSON content: {}", e))
        })?;
        history.push(ConstitutionHistory {
            version,
            treasury_id,
            state_hash,
            content,
            executed_at,
        });
    }

    Ok(Json(history))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:treasury_id/constitution", get(get_current_constitution).post(create_or_update_constitution))
        .route("/:treasury_id/constitution/history", get(get_constitution_history))
        .route("/:treasury_id/constitution/rollback/:version", post(rollback_constitution))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_valid_constitution() {
        let content = ConstitutionContent {
            memo: None,
            pools: vec![
                PoolConfig {
                    pool_key: "core_reserves".to_string(),
                    asset_code: "USDC".to_string(),
                    target_pct: 70.0,
                    floor_pct: 60.0,
                    ceiling_pct: 80.0,
                    drift_bounds_pct: 5.0,
                },
                PoolConfig {
                    pool_key: "growth".to_string(),
                    asset_code: "XLM".to_string(),
                    target_pct: 30.0,
                    floor_pct: 20.0,
                    ceiling_pct: 40.0,
                    drift_bounds_pct: 5.0,
                },
            ],
        };
        let result = validate_constitution(&content);
        assert!(result.valid, "Expected valid, got errors: {:?}", result.errors);
    }

    #[test]
    fn test_validate_invalid_sum() {
        let content = ConstitutionContent {
            memo: None,
            pools: vec![
                PoolConfig {
                    pool_key: "single".to_string(),
                    asset_code: "USDC".to_string(),
                    target_pct: 90.0,
                    floor_pct: 80.0,
                    ceiling_pct: 100.0,
                    drift_bounds_pct: 5.0,
                },
            ],
        };
        let result = validate_constitution(&content);
        assert!(!result.valid);
        assert!(result.errors[0].contains("Total target_pct must equal 100.0"));
    }

    #[test]
    fn test_validate_invalid_bounds() {
        let content = ConstitutionContent {
            memo: None,
            pools: vec![
                PoolConfig {
                    pool_key: "single".to_string(),
                    asset_code: "USDC".to_string(),
                    target_pct: 100.0,
                    floor_pct: 80.0,
                    ceiling_pct: 90.0, // target is above ceiling
                    drift_bounds_pct: 5.0,
                },
            ],
        };
        let result = validate_constitution(&content);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("target (100) must be between floor (80) and ceiling (90)")));
    }
}
