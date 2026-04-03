use synod_coordinator::config::Settings;
use synod_coordinator::{config, AppState};
use redis::aio::ConnectionManager;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use std::net::SocketAddr;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;
use dotenvy::dotenv;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenv().ok();

    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::DEBUG)
        .with_target(false)
        .json()
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    info!("Starting Synod Coordinator...");

    // Load typed configuration
    let settings = Settings::load().unwrap_or_else(|e| {
        // Fallback to env vars if config file not found
        tracing::warn!("Config file not found, falling back to env vars: {}", e);
        Settings {
            server: config::ServerConfig::default(),
            database: config::DatabaseConfig {
                url: std::env::var("DATABASE_URL")
                    .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/synod_db".to_string()),
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
                horizon_url: std::env::var("HORIZON_URL").unwrap_or_else(|_| "https://horizon-testnet.stellar.org".to_string()),
                coordinator_pubkey: std::env::var("SYNOD_STELLAR__COORDINATOR_PUBKEY").unwrap_or_default(),
                coordinator_secret_key: std::env::var("SYNOD_STELLAR__COORDINATOR_SECRET_KEY").unwrap_or_default(),
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

    // Connect to Postgres
    info!("Connecting to Postgres...");
    let db_pool = PgPoolOptions::new()
        .max_connections(settings.database.max_connections)
        .connect(&settings.database.url)
        .await?;

    // Auto-run migrations
    info!("Running database migrations...");
    sqlx::migrate!("./migrations")
        .run(&db_pool)
        .await?;

    // Connect to Redis
    info!("Connecting to Redis at {}...", settings.redis.url);
    let redis_client = redis::Client::open(settings.redis.url.as_str())?;
    let redis_manager = ConnectionManager::new(redis_client).await?;

    let (tx_events, _) = tokio::sync::broadcast::channel(100);

    let state = AppState {
        db: db_pool,
        redis: redis_manager,
        config: settings.clone(),
        tx_events,
    };

    let app = synod_coordinator::router(state.clone());

    // Spawn Background Permit TTL Watcher
    let db_pool_clone = state.db.clone();
    tokio::spawn(async move {
        info!("Permit TTL Watcher started");
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = sqlx::query!(
                "UPDATE permits SET status = 'EXPIRED' WHERE status = 'ACTIVE' AND expires_at < NOW()"
            ).execute(&db_pool_clone).await {
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
            // Flip agents to INACTIVE if no heartbeat for 10 minutes
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

    let addr = SocketAddr::from(([0, 0, 0, 0], settings.server.port));
    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
