use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::{
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::info;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

fn default_max_concurrent_permits() -> i32 {
    10
}

fn default_agent_tier_limit() -> f64 {
    1000.0
}

fn default_agent_concurrent_cap() -> i32 {
    5
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TreasuryRules {
    pub max_drawdown_pct: f64,
    #[serde(default = "default_max_concurrent_permits")]
    pub max_concurrent_permits: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentWalletRule {
    pub agent_id: Uuid,
    pub wallet_address: String,
    pub allocation_pct: f64,
    pub tier_limit_usd: f64,
    pub concurrent_permit_cap: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConstitutionContent {
    pub treasury_rules: TreasuryRules,
    #[serde(default)]
    pub agent_wallet_rules: Vec<AgentWalletRule>,
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

pub fn validate_constitution(content: &ConstitutionContent) -> ValidationResult {
    let mut errors = Vec::new();
    let mut wallet_totals: HashMap<String, f64> = HashMap::new();

    if content.treasury_rules.max_drawdown_pct <= 0.0 {
        errors.push("Treasury max_drawdown_pct must be greater than 0".to_string());
    }

    if content.treasury_rules.max_concurrent_permits < 1 {
        errors.push("Treasury max_concurrent_permits must be at least 1".to_string());
    }

    for rule in &content.agent_wallet_rules {
        if !(1.0..=100.0).contains(&rule.allocation_pct) {
            errors.push(format!(
                "Agent {} on wallet {}: allocation_pct must be between 1 and 100",
                rule.agent_id, rule.wallet_address
            ));
        }

        if rule.tier_limit_usd <= 0.0 {
            errors.push(format!(
                "Agent {} on wallet {}: tier_limit_usd must be greater than 0",
                rule.agent_id, rule.wallet_address
            ));
        }

        if rule.concurrent_permit_cap < 1 {
            errors.push(format!(
                "Agent {} on wallet {}: concurrent_permit_cap must be at least 1",
                rule.agent_id, rule.wallet_address
            ));
        }

        let current_total = wallet_totals
            .entry(rule.wallet_address.clone())
            .or_insert(0.0);
        *current_total += rule.allocation_pct;
    }

    for (wallet, total) in wallet_totals {
        if total > 100.0 {
            errors.push(format!(
                "Wallet {} total allocation exceeds 100.0% (got {:.2}%)",
                wallet, total
            ));
        }
    }

    ValidationResult {
        valid: errors.is_empty(),
        errors,
    }
}

pub fn generate_state_hash(content: &ConstitutionContent) -> AppResult<String> {
    let json_bytes = serde_json::to_vec(content).map_err(|e| {
        AppError::Internal(anyhow::anyhow!(
            "Failed to serialize constitution for hashing: {}",
            e
        ))
    })?;

    let hash = Sha256::digest(&json_bytes);
    Ok(hex::encode(hash))
}

pub fn normalize_constitution_value(
    content_json: serde_json::Value,
) -> AppResult<ConstitutionContent> {
    let memo = content_json
        .get("memo")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    let treasury_rules = if let Some(rules) = content_json.get("treasury_rules") {
        TreasuryRules {
            max_drawdown_pct: rules
                .get("max_drawdown_pct")
                .and_then(|value| value.as_f64())
                .or_else(|| {
                    content_json
                        .get("max_drawdown_pct")
                        .and_then(|value| value.as_f64())
                })
                .unwrap_or(15.0),
            max_concurrent_permits: rules
                .get("max_concurrent_permits")
                .and_then(|value| value.as_i64())
                .map(|value| value as i32)
                .or_else(|| {
                    content_json
                        .get("max_concurrent_permits")
                        .and_then(|value| value.as_i64())
                        .map(|value| value as i32)
                })
                .unwrap_or(default_max_concurrent_permits()),
        }
    } else {
        TreasuryRules {
            max_drawdown_pct: content_json
                .get("max_drawdown_pct")
                .and_then(|value| value.as_f64())
                .unwrap_or(15.0),
            max_concurrent_permits: content_json
                .get("max_concurrent_permits")
                .and_then(|value| value.as_i64())
                .map(|value| value as i32)
                .unwrap_or(default_max_concurrent_permits()),
        }
    };

    let mut agent_wallet_rules = Vec::new();

    if let Some(rules) = content_json
        .get("agent_wallet_rules")
        .and_then(|value| value.as_array())
    {
        for rule in rules {
            let agent_id = rule
                .get("agent_id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    AppError::InvalidInput("agent_wallet_rules[].agent_id missing".into())
                })
                .and_then(|value| {
                    Uuid::parse_str(value)
                        .map_err(|e| AppError::InvalidInput(format!("Invalid agent_id: {}", e)))
                })?;

            let wallet_address = rule
                .get("wallet_address")
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    AppError::InvalidInput("agent_wallet_rules[].wallet_address missing".into())
                })?
                .to_string();

            agent_wallet_rules.push(AgentWalletRule {
                agent_id,
                wallet_address,
                allocation_pct: rule
                    .get("allocation_pct")
                    .and_then(|value| value.as_f64())
                    .unwrap_or(100.0),
                tier_limit_usd: rule
                    .get("tier_limit_usd")
                    .and_then(|value| value.as_f64())
                    .unwrap_or(default_agent_tier_limit()),
                concurrent_permit_cap: rule
                    .get("concurrent_permit_cap")
                    .and_then(|value| value.as_i64())
                    .map(|value| value as i32)
                    .unwrap_or(default_agent_concurrent_cap()),
            });
        }
    } else if let Some(legacy_allocations) = content_json
        .get("agent_allocations")
        .and_then(|value| value.as_array())
    {
        for allocation in legacy_allocations {
            let Some(agent_id) = allocation.get("agent_id").and_then(|value| value.as_str()) else {
                continue;
            };
            let Some(wallet_address) = allocation
                .get("wallet_address")
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            let Ok(agent_id) = Uuid::parse_str(agent_id) else {
                continue;
            };

            agent_wallet_rules.push(AgentWalletRule {
                agent_id,
                wallet_address: wallet_address.to_string(),
                allocation_pct: allocation
                    .get("allocation_pct")
                    .and_then(|value| value.as_f64())
                    .unwrap_or(100.0),
                tier_limit_usd: default_agent_tier_limit(),
                concurrent_permit_cap: default_agent_concurrent_cap(),
            });
        }
    }

    Ok(ConstitutionContent {
        treasury_rules,
        agent_wallet_rules,
        memo,
    })
}

pub fn rules_for_agent(content: &ConstitutionContent, agent_id: Uuid) -> Vec<AgentWalletRule> {
    content
        .agent_wallet_rules
        .iter()
        .filter(|rule| rule.agent_id == agent_id)
        .cloned()
        .collect()
}

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
        "#,
    )
    .bind(treasury_id)
    .fetch_optional(&state.db)
    .await?;

    match raw {
        Some((version, state_hash, content_json, executed_at)) => Ok(Json(ConstitutionHistory {
            version,
            treasury_id,
            state_hash,
            content: normalize_constitution_value(content_json)?,
            executed_at,
        })),
        None => Err(AppError::NotFound("Constitution not found".to_string())),
    }
}

pub async fn create_or_update_constitution(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<CreateConstitutionRequest>,
) -> AppResult<(StatusCode, Json<ConstitutionHistory>)> {
    info!(treasury = %treasury_id, "Hit create_or_update_constitution");

    let validation = validate_constitution(&payload.content);
    if !validation.valid {
        return Err(AppError::InvalidInput(validation.errors.join(", ")));
    }

    let hash = generate_state_hash(&payload.content)?;
    let max_v: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) FROM constitution_history WHERE treasury_id = $1",
    )
    .bind(treasury_id)
    .fetch_one(&state.db)
    .await?;
    let next_version = max_v + 1;
    let content_json = serde_json::to_value(&payload.content).unwrap();

    let mut tx = state.db.begin().await?;

    sqlx::query(
        r#"
        INSERT INTO constitution_history (treasury_id, version, state_hash, content)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(treasury_id)
    .bind(next_version)
    .bind(&hash)
    .bind(&content_json)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "UPDATE treasuries SET constitution_version = $1, updated_at = $2 WHERE treasury_id = $3",
    )
    .bind(next_version)
    .bind(Utc::now())
    .bind(treasury_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    info!(treasury = %treasury_id, version = %next_version, "Constitution updated");

    use redis::AsyncCommands;
    let mut redis = state.redis.clone();
    let cache_key = format!("constitution:{}", treasury_id);
    let _: () = redis
        .set(&cache_key, serde_json::to_string(&payload.content).unwrap())
        .await
        .unwrap_or(());

    let res = get_current_constitution(State(state.clone()), auth, Path(treasury_id)).await?;

    let _ = state
        .tx_events
        .send(crate::TreasuryEvent::ConstitutionUpdate {
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
    let old_raw: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT content FROM constitution_history WHERE treasury_id = $1 AND version = $2",
    )
    .bind(treasury_id)
    .bind(target_version)
    .fetch_optional(&state.db)
    .await?;

    let content_json = old_raw
        .ok_or_else(|| AppError::NotFound("Target version not found".to_string()))?
        .0;
    let content = normalize_constitution_value(content_json)?;

    create_or_update_constitution(
        State(state),
        auth,
        Path(treasury_id),
        Json(CreateConstitutionRequest { content }),
    )
    .await
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
        "#,
    )
    .bind(treasury_id)
    .fetch_all(&state.db)
    .await?;

    let mut history = Vec::new();
    for (version, state_hash, content_json, executed_at) in rows {
        history.push(ConstitutionHistory {
            version,
            treasury_id,
            state_hash,
            content: normalize_constitution_value(content_json)?,
            executed_at,
        });
    }

    Ok(Json(history))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/:treasury_id/constitution",
            get(get_current_constitution)
                .post(create_or_update_constitution)
                .put(create_or_update_constitution),
        )
        .route(
            "/:treasury_id/constitution/history",
            get(get_constitution_history),
        )
        .route(
            "/:treasury_id/constitution/rollback/:version",
            post(rollback_constitution),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_valid_constitution() {
        let content = ConstitutionContent {
            memo: None,
            treasury_rules: TreasuryRules {
                max_drawdown_pct: 15.0,
                max_concurrent_permits: 10,
            },
            agent_wallet_rules: vec![
                AgentWalletRule {
                    agent_id: Uuid::new_v4(),
                    wallet_address: "WALLET_1".to_string(),
                    allocation_pct: 70.0,
                    tier_limit_usd: 5000.0,
                    concurrent_permit_cap: 3,
                },
                AgentWalletRule {
                    agent_id: Uuid::new_v4(),
                    wallet_address: "WALLET_1".to_string(),
                    allocation_pct: 30.0,
                    tier_limit_usd: 5000.0,
                    concurrent_permit_cap: 3,
                },
            ],
        };
        let result = validate_constitution(&content);
        assert!(
            result.valid,
            "Expected valid, got errors: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_normalize_legacy_constitution() {
        let content = serde_json::json!({
            "agent_allocations": [{
                "agent_id": Uuid::new_v4(),
                "wallet_address": "GABC",
                "allocation_pct": 50.0
            }],
            "max_drawdown_pct": 12.5,
            "memo": "legacy"
        });

        let normalized = normalize_constitution_value(content).unwrap();
        assert_eq!(normalized.treasury_rules.max_drawdown_pct, 12.5);
        assert_eq!(normalized.treasury_rules.max_concurrent_permits, 10);
        assert_eq!(normalized.agent_wallet_rules.len(), 1);
    }
}
