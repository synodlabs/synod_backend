mod config;
mod error;

use axum::{routing::get, Router};
use config::Settings;
use redis::aio::ConnectionManager;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::net::SocketAddr;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;
use dotenvy::dotenv;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub redis: ConnectionManager,
    pub config: Settings,
}

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
                network: "testnet".to_string(),
                horizon_url: "https://horizon-testnet.stellar.org".to_string(),
                coordinator_pubkey: String::new(),
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

    // Connect to Redis
    info!("Connecting to Redis at {}...", settings.redis.url);
    let redis_client = redis::Client::open(settings.redis.url.as_str())?;
    let redis_manager = ConnectionManager::new(redis_client).await?;

    let state = AppState {
        db: db_pool,
        redis: redis_manager,
        config: settings.clone(),
    };

    let app = Router::new()
        .route("/health", get(health_check))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], settings.server.port));
    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}
