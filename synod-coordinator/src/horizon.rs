use crate::error::{AppError, AppResult};
use crate::AppState;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{info, warn, error, debug};
use uuid::Uuid;

// ── Constants ──
const CURSOR_TTL_SECS: u64 = 604_800; // 7 days
const DEDUP_TTL_SECS: u64 = 172_800;  // 48 hours
const DEDUP_MAX_ENTRIES: usize = 2_000;
const HEARTBEAT_TIMEOUT_SECS: u64 = 30;
const MAX_BACKOFF_SECS: u64 = 60;
const STAGGER_DELAY_SECS: u64 = 2;

// ── Redis Key Helpers ──
fn cursor_key(wallet: &str) -> String {
    format!("horizon:cursor:{}", wallet)
}

fn dedup_key(wallet: &str) -> String {
    format!("horizon:seen:{}", wallet)
}

// ── Reconnection Strategy ──
#[derive(Debug, Clone, PartialEq)]
pub enum ReconnectStrategy {
    Immediate,
    FixedDelay(Duration),
    ExponentialBackoff { attempt: u32, base_secs: u64, max_secs: u64 },
    RetryAfter(Duration),
    PermanentPause,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DisconnectReason {
    CleanEof,
    SilentDrop,
    NetworkError,
    RateLimited,
    HorizonDown,
    AccountDeleted,
}

impl DisconnectReason {
    pub fn strategy(&self, attempt: u32) -> ReconnectStrategy {
        match self {
            DisconnectReason::CleanEof => ReconnectStrategy::FixedDelay(Duration::from_secs(1)),
            DisconnectReason::SilentDrop => ReconnectStrategy::Immediate,
            DisconnectReason::NetworkError => ReconnectStrategy::ExponentialBackoff {
                attempt,
                base_secs: 1,
                max_secs: MAX_BACKOFF_SECS,
            },
            DisconnectReason::RateLimited => ReconnectStrategy::RetryAfter(Duration::from_secs(60)),
            DisconnectReason::HorizonDown => ReconnectStrategy::ExponentialBackoff {
                attempt,
                base_secs: 30,
                max_secs: 300,
            },
            DisconnectReason::AccountDeleted => ReconnectStrategy::PermanentPause,
        }
    }
}

// ── SSE Event Models ──
#[derive(Debug, Deserialize, Clone)]
pub struct HorizonOperation {
    pub id: String,
    pub paging_token: String,
    #[serde(rename = "type")]
    pub op_type: String,
    pub source_account: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub amount: Option<String>,
    pub asset_code: Option<String>,
    pub asset_issuer: Option<String>,
    pub asset_type: Option<String>,
    pub created_at: Option<String>,
}

// ── Inflow Classification ──
#[derive(Debug, Clone, PartialEq)]
pub enum InflowResult {
    Discarded,
    UnknownAsset { asset_code: String },
    Routed { pool_key: String, amount: String, asset_code: String },
    UnroutedInflow,
}

pub fn classify_inflow(
    op: &HorizonOperation,
    wallet_address: &str,
    _pool_assets: &[String], // simplified: list of known asset codes
) -> InflowResult {
    // Step 1: Is operation type payment, path_payment, or create_account?
    let valid_types = ["payment", "path_payment_strict_receive", "create_account"];
    if !valid_types.contains(&op.op_type.as_str()) {
        return InflowResult::Discarded;
    }

    // Step 2: Is destination a monitored wallet?
    let destination = op.to.as_deref().unwrap_or("");
    if destination != wallet_address {
        return InflowResult::Discarded;
    }

    // Step 3: Is asset in any pool definition?
    let asset_code = op.asset_code.as_deref().unwrap_or("XLM");
    if !_pool_assets.iter().any(|a| a == asset_code) {
        return InflowResult::UnknownAsset {
            asset_code: asset_code.to_string(),
        };
    }

    // Steps 4-7: Route to pool (simplified: use asset code as pool key)
    InflowResult::Routed {
        pool_key: format!("pool:{}", asset_code),
        amount: op.amount.clone().unwrap_or_default(),
        asset_code: asset_code.to_string(),
    }
}

// ── HorizonWatcher ──
pub struct HorizonWatcher {
    pub wallet_address: String,
    pub treasury_id: Uuid,
    pub state: AppState,
    pub horizon_url: String,
    consecutive_failures: u32,
}

impl HorizonWatcher {
    pub fn new(wallet_address: String, treasury_id: Uuid, state: AppState) -> Self {
        let horizon_url = state.config.stellar.horizon_url.clone();
        Self {
            wallet_address,
            treasury_id,
            state,
            horizon_url,
            consecutive_failures: 0,
        }
    }

    // ── Cursor Management ──
    pub async fn get_cursor(&self) -> String {
        let mut redis = self.state.redis.clone();
        let key = cursor_key(&self.wallet_address);
        redis.get::<_, Option<String>>(&key)
            .await
            .unwrap_or(None)
            .unwrap_or_else(|| "now".to_string())
    }

    pub async fn save_cursor(&self, cursor: &str) {
        let mut redis = self.state.redis.clone();
        let key = cursor_key(&self.wallet_address);
        let _: redis::RedisResult<()> = redis.set_ex(&key, cursor, CURSOR_TTL_SECS).await;
    }

    // ── Deduplication ──
    pub async fn is_duplicate(&self, operation_id: &str) -> bool {
        let mut redis = self.state.redis.clone();
        let key = dedup_key(&self.wallet_address);
        redis.sismember(&key, operation_id)
            .await
            .unwrap_or(false)
    }

    pub async fn mark_seen(&self, operation_id: &str) {
        let mut redis = self.state.redis.clone();
        let key = dedup_key(&self.wallet_address);
        let _: redis::RedisResult<()> = redis.sadd(&key, operation_id).await;
        // Set TTL on the set (refreshed each time)
        let _: redis::RedisResult<()> = redis.expire(&key, DEDUP_TTL_SECS as i64).await;
        // Trim to max entries if needed (approximate via SCARD + SPOP)
        let card: usize = redis.scard(&key).await.unwrap_or(0);
        if card > DEDUP_MAX_ENTRIES {
            let excess = card - DEDUP_MAX_ENTRIES;
            for _ in 0..excess {
                let _: redis::RedisResult<Option<String>> = redis.spop(&key).await;
            }
        }
    }

    // ── Process Operation ──
    async fn process_operation(&self, op: HorizonOperation) {
        // Check dedup
        if self.is_duplicate(&op.id).await {
            debug!(wallet = %self.wallet_address, op_id = %op.id, "Duplicate operation, discarding");
            return;
        }

        // Mark as seen
        self.mark_seen(&op.id).await;

        // Save cursor
        self.save_cursor(&op.paging_token).await;

        // Classify
        let pool_assets = vec!["XLM".to_string(), "USDC".to_string()]; // TODO: fetch from constitution
        let result = classify_inflow(&op, &self.wallet_address, &pool_assets);

        match result {
            InflowResult::Discarded => {
                debug!(wallet = %self.wallet_address, op_type = %op.op_type, "Operation discarded");
            }
            InflowResult::UnknownAsset { asset_code } => {
                warn!(wallet = %self.wallet_address, asset = %asset_code, "Unknown asset received");
            }
            InflowResult::Routed { pool_key, amount, asset_code } => {
                info!(
                    wallet = %self.wallet_address,
                    pool = %pool_key,
                    amount = %amount,
                    asset = %asset_code,
                    "Inflow routed to pool"
                );
            }
            InflowResult::UnroutedInflow => {
                warn!(wallet = %self.wallet_address, "Unrouted inflow detected");
            }
        }
    }

    // ── Main SSE Loop ──
    pub async fn run(&mut self) {
        info!(wallet = %self.wallet_address, "Starting HorizonWatcher");

        loop {
            let cursor = self.get_cursor().await;
            let url = format!(
                "{}/accounts/{}/operations?cursor={}&limit=200",
                self.horizon_url, self.wallet_address, cursor
            );

            info!(wallet = %self.wallet_address, cursor = %cursor, "Connecting to SSE stream");

            match self.stream_events(&url).await {
                Ok(reason) => {
                    info!(wallet = %self.wallet_address, reason = ?reason, "Stream disconnected");
                    let strategy = reason.strategy(self.consecutive_failures);
                    
                    if reason == DisconnectReason::AccountDeleted {
                        error!(wallet = %self.wallet_address, "Account deleted, pausing permanently");
                        break;
                    }

                    if reason == DisconnectReason::CleanEof || reason == DisconnectReason::SilentDrop {
                        self.consecutive_failures = 0;
                    } else {
                        self.consecutive_failures += 1;
                    }

                    match strategy {
                        ReconnectStrategy::Immediate => {}
                        ReconnectStrategy::FixedDelay(d) => sleep(d).await,
                        ReconnectStrategy::ExponentialBackoff { attempt, base_secs, max_secs } => {
                            let delay = std::cmp::min(base_secs * 2u64.pow(attempt), max_secs);
                            sleep(Duration::from_secs(delay)).await;
                        }
                        ReconnectStrategy::RetryAfter(d) => sleep(d).await,
                        ReconnectStrategy::PermanentPause => break,
                    }
                }
                Err(e) => {
                    error!(wallet = %self.wallet_address, error = %e, "Stream error");
                    self.consecutive_failures += 1;
                    let delay = std::cmp::min(1 * 2u64.pow(self.consecutive_failures), MAX_BACKOFF_SECS);
                    sleep(Duration::from_secs(delay)).await;
                }
            }
        }
    }

    async fn stream_events(&self, url: &str) -> Result<DisconnectReason, anyhow::Error> {
        use futures::StreamExt;
        use reqwest_eventsource::{EventSource, Event};

        let mut es = EventSource::get(url);

        loop {
            let event = timeout(
                Duration::from_secs(HEARTBEAT_TIMEOUT_SECS),
                es.next(),
            ).await;

            match event {
                Err(_) => {
                    // Timeout — silent drop
                    es.close();
                    return Ok(DisconnectReason::SilentDrop);
                }
                Ok(None) => {
                    // Stream ended cleanly
                    return Ok(DisconnectReason::CleanEof);
                }
                Ok(Some(result)) => {
                    match result {
                        Ok(Event::Open) => {
                            debug!(wallet = %self.wallet_address, "SSE stream opened");
                        }
                        Ok(Event::Message(msg)) => {
                            if msg.event == "message" {
                                match serde_json::from_str::<HorizonOperation>(&msg.data) {
                                    Ok(op) => self.process_operation(op).await,
                                    Err(e) => {
                                        warn!(wallet = %self.wallet_address, error = %e, "Failed to parse operation");
                                    }
                                }
                            }
                        }
                        Err(reqwest_eventsource::Error::StreamEnded) => {
                            return Ok(DisconnectReason::CleanEof);
                        }
                        Err(reqwest_eventsource::Error::InvalidStatusCode(code, _)) => {
                            match code.as_u16() {
                                429 => return Ok(DisconnectReason::RateLimited),
                                503 => return Ok(DisconnectReason::HorizonDown),
                                404 => return Ok(DisconnectReason::AccountDeleted),
                                _ => return Err(anyhow::anyhow!("HTTP {}", code)),
                            }
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("SSE error: {}", e));
                        }
                    }
                }
            }
        }
    }
}

// ── Spawn watcher with thundering herd prevention ──
pub async fn spawn_watchers(state: AppState) {
    let wallets: Vec<(String, Uuid)> = sqlx::query_as::<_, (String, Uuid)>(
        "SELECT wallet_address, treasury_id FROM treasury_wallets WHERE status = 'ACTIVE' ORDER BY wallet_address"
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    info!(count = wallets.len(), "Spawning HorizonWatchers with staggered delay");

    for (idx, (wallet_address, treasury_id)) in wallets.into_iter().enumerate() {
        let state_clone = state.clone();
        let delay = Duration::from_secs(idx as u64 * STAGGER_DELAY_SECS);

        tokio::spawn(async move {
            sleep(delay).await;
            let mut watcher = HorizonWatcher::new(wallet_address, treasury_id, state_clone);
            watcher.run().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_inflow_payment_to_monitored_wallet() {
        let op = HorizonOperation {
            id: "1".to_string(),
            paging_token: "123".to_string(),
            op_type: "payment".to_string(),
            source_account: Some("GSENDER...".to_string()),
            from: Some("GSENDER...".to_string()),
            to: Some("GWALLET...".to_string()),
            amount: Some("100.0".to_string()),
            asset_code: Some("USDC".to_string()),
            asset_issuer: Some("GISSUER...".to_string()),
            asset_type: Some("credit_alphanum4".to_string()),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
        };

        let result = classify_inflow(&op, "GWALLET...", &["USDC".to_string(), "XLM".to_string()]);
        assert!(matches!(result, InflowResult::Routed { .. }));
    }

    #[test]
    fn test_classify_inflow_wrong_destination() {
        let op = HorizonOperation {
            id: "2".to_string(),
            paging_token: "124".to_string(),
            op_type: "payment".to_string(),
            source_account: None,
            from: None,
            to: Some("GOTHER...".to_string()),
            amount: Some("50.0".to_string()),
            asset_code: Some("XLM".to_string()),
            asset_issuer: None,
            asset_type: None,
            created_at: None,
        };

        let result = classify_inflow(&op, "GWALLET...", &["XLM".to_string()]);
        assert_eq!(result, InflowResult::Discarded);
    }

    #[test]
    fn test_classify_inflow_unknown_asset() {
        let op = HorizonOperation {
            id: "3".to_string(),
            paging_token: "125".to_string(),
            op_type: "payment".to_string(),
            source_account: None,
            from: None,
            to: Some("GWALLET...".to_string()),
            amount: Some("10.0".to_string()),
            asset_code: Some("DOGE".to_string()),
            asset_issuer: None,
            asset_type: None,
            created_at: None,
        };

        let result = classify_inflow(&op, "GWALLET...", &["XLM".to_string(), "USDC".to_string()]);
        assert!(matches!(result, InflowResult::UnknownAsset { .. }));
    }

    #[test]
    fn test_classify_inflow_non_payment_type() {
        let op = HorizonOperation {
            id: "4".to_string(),
            paging_token: "126".to_string(),
            op_type: "change_trust".to_string(),
            source_account: None,
            from: None,
            to: None,
            amount: None,
            asset_code: None,
            asset_issuer: None,
            asset_type: None,
            created_at: None,
        };

        let result = classify_inflow(&op, "GWALLET...", &["XLM".to_string()]);
        assert_eq!(result, InflowResult::Discarded);
    }

    #[test]
    fn test_reconnect_strategies() {
        assert_eq!(
            DisconnectReason::CleanEof.strategy(0),
            ReconnectStrategy::FixedDelay(Duration::from_secs(1))
        );
        assert_eq!(
            DisconnectReason::SilentDrop.strategy(0),
            ReconnectStrategy::Immediate
        );
        assert_eq!(
            DisconnectReason::AccountDeleted.strategy(0),
            ReconnectStrategy::PermanentPause
        );
    }
}
