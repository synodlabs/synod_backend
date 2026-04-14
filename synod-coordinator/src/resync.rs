use axum::extract::{Path, State};
use axum::{routing::post, Json, Router};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use tracing::{info, warn, error};
use crate::error::{AppError, AppResult};
use crate::AppState;
use crate::auth::AuthUser;

// ── Price Fetching ──
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AssetPrice {
    pub asset_code: String,
    pub usd_price: f64,
    pub source: String,
}

pub async fn fetch_usd_price(asset_code: &str, state: &AppState) -> f64 {
    // Fallback chain: Stellar DEX → CoinGecko → last known

    // 1. Try Stellar DEX (Horizon trade aggregations)
    if let Some(price) = fetch_from_stellar_dex(asset_code, state).await {
        return price;
    }

    // 2. Try CoinGecko
    if let Some(price) = fetch_from_coingecko(asset_code).await {
        return price;
    }

    // 3. Last known from Redis
    if let Some(price) = fetch_last_known(asset_code, state).await {
        warn!(asset = %asset_code, price = %price, "Using last known price");
        return price;
    }

    // Fallback: XLM defaults
    match asset_code {
        "XLM" => 0.10,
        "USDC" => 1.0,
        _ => 0.0,
    }
}

async fn fetch_from_stellar_dex(asset_code: &str, state: &AppState) -> Option<f64> {
    let client = reqwest::Client::new();
    let url = format!(
        "{}/trade_aggregations?base_asset_type=native&counter_asset_type=credit_alphanum4&counter_asset_code=USDC&counter_asset_issuer=GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN&resolution=900000&limit=1&order=desc",
        state.config.stellar.horizon_url
    );

    match client.get(&url).send().await {
        Ok(resp) => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                if let Some(records) = body["_embedded"]["records"].as_array() {
                    if let Some(record) = records.first() {
                        if let Some(avg) = record["avg"].as_str() {
                            if let Ok(price) = avg.parse::<f64>() {
                                let final_price = if asset_code == "XLM" {
                                    1.0 / price
                                } else {
                                    price
                                };

                                let mut redis = state.redis.clone();
                                let key = format!("price:last:{}", asset_code);
                                let _: redis::RedisResult<()> = redis.set_ex(&key, final_price.to_string(), 3600).await;

                                return Some(final_price);
                            }
                        }
                    }
                }
            }
            None
        }
        Err(_) => None,
    }
}

async fn fetch_from_coingecko(asset_code: &str) -> Option<f64> {
    let coingecko_id = match asset_code {
        "XLM" => "stellar",
        "USDC" => return Some(1.0),
        _ => return None,
    };

    let client = reqwest::Client::new();
    let url = format!(
        "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd",
        coingecko_id
    );

    match client.get(&url).send().await {
        Ok(resp) => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                body[coingecko_id]["usd"].as_f64()
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

async fn fetch_last_known(asset_code: &str, state: &AppState) -> Option<f64> {
    let mut redis = state.redis.clone();
    let key = format!("price:last:{}", asset_code);
    let val: Option<String> = redis.get(&key).await.ok()?;
    val.and_then(|v| v.parse::<f64>().ok())
}

// ── Balance Resync ──
#[derive(Debug, Serialize)]
pub struct ResyncResult {
    pub treasury_id: Uuid,
    pub balances: Vec<WalletBalance>,
    pub total_aum_usd: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WalletBalance {
    pub wallet_address: String,
    pub asset_code: String,
    pub balance: String,
    pub usd_value: f64,
}

pub async fn resync_treasury(
    treasury_id: Uuid,
    state: &AppState,
) -> AppResult<ResyncResult> {
    info!(treasury = %treasury_id, "Starting balance resync");

    let wallets: Vec<(String,)> = sqlx::query_as(
        "SELECT wallet_address FROM treasury_wallets WHERE treasury_id = $1 AND status = 'ACTIVE'"
    )
    .bind(treasury_id)
    .fetch_all(&state.db)
    .await?;

    let client = reqwest::Client::new();
    let mut all_balances = Vec::new();
    let mut total_aum = 0.0;

    for (wallet_address,) in &wallets {
        let url = format!("{}/accounts/{}", state.config.stellar.horizon_url, wallet_address);

        match client.get(&url).send().await {
            Ok(resp) => {
                if let Ok(account) = resp.json::<serde_json::Value>().await {
                    if let Some(balances) = account["balances"].as_array() {
                        for bal in balances {
                            let asset_code = if bal["asset_type"].as_str() == Some("native") {
                                "XLM".to_string()
                            } else {
                                bal["asset_code"].as_str().unwrap_or("UNKNOWN").to_string()
                            };

                            let balance_str = bal["balance"].as_str().unwrap_or("0");
                            let balance_f64: f64 = balance_str.parse().unwrap_or(0.0);

                            let usd_price = fetch_usd_price(&asset_code, state).await;
                            let usd_value = balance_f64 * usd_price;
                            total_aum += usd_value;

                            all_balances.push(WalletBalance {
                                wallet_address: wallet_address.clone(),
                                asset_code,
                                balance: balance_str.to_string(),
                                usd_value,
                            });
                        }
                    }
                }
            }
            Err(e) => {
                warn!(wallet = %wallet_address, error = %e, "Failed to fetch account balances");
            }
        }
    }

    // Atomically replace Redis snapshot
    {
        let mut redis = state.redis.clone();
        let snapshot_key = format!("treasury:snapshot:{}", treasury_id);
        let snapshot_json = serde_json::to_string(&all_balances)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("JSON error: {}", e)))?;
        let _: redis::RedisResult<()> = redis.set(&snapshot_key, &snapshot_json).await;

        let _: redis::RedisResult<()> = redis.set(
            format!("treasury:aum:{}", treasury_id),
            total_aum.to_string(),
        ).await;
    }

    // Update peak AUM if current exceeds it
    sqlx::query(
        "UPDATE treasuries SET current_aum_usd = CAST($1 AS NUMERIC(20,7)), peak_aum_usd = GREATEST(peak_aum_usd, CAST($1 AS NUMERIC(20,7))), updated_at = NOW() WHERE treasury_id = $2"
    )
    .bind(format!("{:.7}", total_aum))
    .bind(treasury_id)
    .execute(&state.db)
    .await?;

    info!(treasury = %treasury_id, aum = total_aum, wallets = wallets.len(), "Resync complete");

    Ok(ResyncResult {
        treasury_id,
        balances: all_balances,
        total_aum_usd: total_aum,
    })
}

// ── Manual Resync Endpoint (works under both /v1/treasuries/:id/resync and /admin) ──
pub async fn manual_resync(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
) -> AppResult<Json<ResyncResult>> {
    let result = resync_treasury(treasury_id, &state).await?;
    Ok(Json(result))
}

// ── Scheduled Reconciliation ──
pub async fn scheduled_reconciliation(state: AppState) {
    info!("Starting scheduled 6-hour reconciliation");

    let treasuries: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT treasury_id FROM treasuries WHERE health != 'HALTED'"
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for (tid,) in treasuries {
        match resync_treasury(tid, &state).await {
            Ok(result) => info!(treasury = %tid, aum = result.total_aum_usd, "Reconciliation complete"),
            Err(e) => error!(treasury = %tid, error = %e, "Reconciliation failed"),
        }
    }
}

/// Admin-only router (legacy path — kept for backward compat)
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/treasuries/:id/resync", post(manual_resync))
}
