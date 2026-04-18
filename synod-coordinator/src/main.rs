use dotenvy::dotenv;
use redis::aio::ConnectionManager;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use synod_coordinator::config::Settings;
use synod_coordinator::{config, AppState, WatcherHandles};
use tokio::sync::Mutex;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

fn is_local_connection(url: &str) -> bool {
    let normalized = url.trim().to_ascii_lowercase();
    normalized.contains("localhost")
        || normalized.contains("127.0.0.1")
        || normalized.contains("@postgres:5432")
        || normalized.contains("@redis:6379")
}

fn validate_hosted_runtime_config(settings: &Settings) -> anyhow::Result<()> {
    let is_hosted_runtime = std::env::var("PORT")
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);

    if !is_hosted_runtime {
        return Ok(());
    }

    if is_local_connection(&settings.database.url) {
        anyhow::bail!(
            "DATABASE_URL is still pointing to a local address. In Render, set DATABASE_URL to your Render Postgres internal connection string."
        );
    }

    if is_local_connection(&settings.redis.url) {
        anyhow::bail!(
            "REDIS_URL is still pointing to a local address. In Render, set REDIS_URL to your Render Key Value internal connection string."
        );
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::DEBUG)
        .with_target(false)
        .json()
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    info!("Starting Synod Coordinator...");

    // Load typed configuration
    let settings = Settings::load().unwrap_or_else(|e| {
        tracing::warn!("Config file not found, falling back to env vars: {}", e);
        Settings {
            server: config::ServerConfig::default(),
            database: config::DatabaseConfig {
                url: std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                    "postgres://postgres:postgres@localhost:5432/synod_db".to_string()
                }),
                max_connections: 20,
            },
            redis: config::RedisConfig {
                url: std::env::var("REDIS_URL")
                    .unwrap_or_else(|_| "redis://localhost:6379".to_string()),
            },
            stellar: config::StellarConfig {
                network: std::env::var("STELLAR_NETWORK").unwrap_or_else(|_| "testnet".to_string()),
                network_passphrase: std::env::var("SYNOD_STELLAR__NETWORK_PASSPHRASE")
                    .unwrap_or_else(|_| "Test SDF Network ; September 2015".to_string()),
                horizon_url: std::env::var("HORIZON_URL")
                    .unwrap_or_else(|_| "https://horizon-testnet.stellar.org".to_string()),
                coordinator_pubkey: std::env::var("SYNOD_STELLAR__COORDINATOR_PUBKEY")
                    .unwrap_or_default(),
                coordinator_secret_key: std::env::var("SYNOD_STELLAR__COORDINATOR_SECRET_KEY")
                    .unwrap_or_default(),
                coordinator_secret_key_path: String::new(),
            },
            auth: config::AuthConfig {
                jwt_secret: std::env::var("JWT_SECRET").unwrap_or_default(),
                jwt_expiry_hours: synod_shared::consts::DEFAULT_JWT_EXPIRY_HOURS,
                bcrypt_cost: synod_shared::consts::DEFAULT_BCRYPT_COST,
            },
            walletconnect: config::WalletConnectConfig {
                project_id: String::new(),
                relay_url: "wss://relay.walletconnect.com".to_string(),
            },
        }
    });

    validate_hosted_runtime_config(&settings)?;

    // Connect to Postgres
    info!("Connecting to Postgres...");
    let db_pool = PgPoolOptions::new()
        .max_connections(settings.database.max_connections)
        .connect(&settings.database.url)
        .await?;

    // Auto-run migrations
    info!("Running database migrations...");
    sqlx::migrate!("./migrations").run(&db_pool).await?;

    // Connect to Redis
    info!("Connecting to Redis at {}...", settings.redis.url);
    let redis_client = redis::Client::open(settings.redis.url.as_str())?;
    let redis_manager = ConnectionManager::new(redis_client).await?;

    let (tx_events, _) = tokio::sync::broadcast::channel(100);

    let watcher_handles: WatcherHandles = Arc::new(Mutex::new(HashMap::new()));

    let state = AppState {
        db: db_pool,
        redis: redis_manager,
        config: settings.clone(),
        tx_events,
        watcher_handles,
    };

    let app = synod_coordinator::router(state.clone());

    // Spawn Background Permit TTL Watcher
    let db_pool_clone = state.db.clone();
    tokio::spawn(async move {
        info!("Permit TTL Watcher started");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = sqlx::query(
                "UPDATE permits SET status = 'EXPIRED' WHERE status = 'ACTIVE' AND expires_at < NOW()"
            )
            .execute(&db_pool_clone)
            .await
            {
                tracing::error!("Failed to expire permits: {}", e);
            }
        }
    });

    // Spawn Background Agent Heartbeat Monitor
    let db_pool_hb = state.db.clone();
    let tx_events_hb = state.tx_events.clone();
    tokio::spawn(async move {
        info!("Agent Heartbeat Monitor started");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(120));
        loop {
            interval.tick().await;
            match sqlx::query(
                "UPDATE agent_slots SET status = 'INACTIVE' WHERE status = 'ACTIVE' AND last_connected < NOW() - INTERVAL '10 minutes' RETURNING agent_id, treasury_id"
            )
            .fetch_all(&db_pool_hb)
            .await {
                Ok(rows) => {
                    for row in rows {
                        let agent_id: uuid::Uuid = row.get("agent_id");
                        let treasury_id: uuid::Uuid = row.get("treasury_id");
                        info!(agent = %agent_id, "Agent flipped to INACTIVE (heartbeat timeout)");
                        let _ = tx_events_hb.send(synod_coordinator::TreasuryEvent::AgentStatusChanged {
                            treasury_id,
                            agent_id,
                            new_status: "INACTIVE".to_string(),
                        });
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to check agent heartbeats: {}", e);
                }
            }
        }
    });

    // Spawn Horizon Watchers for all active wallets
    synod_coordinator::horizon::spawn_watchers(state.clone()).await;

    // Spawn 6-hour scheduled reconciliation
    let recon_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
        interval.tick().await; // skip first immediate tick
        loop {
            interval.tick().await;
            synod_coordinator::resync::scheduled_reconciliation(recon_state.clone()).await;
        }
    });

    let addr = SocketAddr::from(([0, 0, 0, 0], settings.server.port));
    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
