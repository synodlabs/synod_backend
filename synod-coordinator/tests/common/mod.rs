use synod_coordinator::config::Settings;
use sqlx::postgres::PgPoolOptions;
use synod_coordinator::AppState;
use redis::aio::ConnectionManager;
use tokio::net::TcpListener;

pub async fn spawn_test_server() -> String {
    let settings = Settings {
        server: synod_coordinator::config::ServerConfig { 
            host: "127.0.0.1".to_string(),
            port: 0 
        },
        database: synod_coordinator::config::DatabaseConfig {
            url: "postgres://postgres:postgres@localhost:5432/synod_db".to_string(),
            max_connections: 5,
        },
        redis: synod_coordinator::config::RedisConfig {
            url: "redis://localhost:6379".to_string(),
        },
        stellar: synod_coordinator::config::StellarConfig {
            network: "testnet".to_string(),
            horizon_url: "https://horizon-testnet.stellar.org".to_string(),
            coordinator_pubkey: "GD...".to_string(),
            coordinator_secret_key_path: "".to_string(),
        },
        auth: synod_coordinator::config::AuthConfig {
            jwt_secret: "test_secret_32_bytes_long_minimum!!".to_string(),
            jwt_expiry_hours: 1,
            bcrypt_cost: 4,
        },
        walletconnect: synod_coordinator::config::WalletConnectConfig {
            project_id: "".to_string(),
            relay_url: "".to_string(),
        },
    };

    let db_pool = PgPoolOptions::new()
        .connect(&settings.database.url)
        .await
        .unwrap();

    // Aggressively wipe the schema to resolve migration history issues in tests
    sqlx::query("DROP SCHEMA public CASCADE").execute(&db_pool).await.unwrap();
    sqlx::query("CREATE SCHEMA public").execute(&db_pool).await.unwrap();

    // Run migrations so the schema is fresh
    sqlx::migrate!("./migrations")
        .run(&db_pool)
        .await
        .unwrap();

    let redis_client = redis::Client::open(settings.redis.url.as_str()).unwrap();
    let mut redis_client_conn = redis_client.get_connection().unwrap();
    redis::cmd("FLUSHALL").query::<()>(&mut redis_client_conn).unwrap();

    let redis_manager = ConnectionManager::new(redis_client).await.unwrap();

    let state = AppState {
        db: db_pool,
        redis: redis_manager,
        config: settings,
    };

    let app = synod_coordinator::router(state);
    
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{}", addr)
}

pub fn generate_test_stellar_keypair() -> (ed25519_dalek::SigningKey, String) {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    
    // Encode as Stellar G... address
    let mut raw = vec![0x30u8];
    raw.extend_from_slice(&verifying_key.to_bytes());
    raw.extend_from_slice(&[0, 0]); 
    
    let pk_stellar = data_encoding::BASE32_NOPAD.encode(&raw);
    (signing_key, pk_stellar)
}

pub fn sign_with_key(signing_key: &ed25519_dalek::SigningKey, message: &[u8]) -> String {
    use ed25519_dalek::Signer;
    let signature = signing_key.sign(message);
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, signature.to_bytes())
}
