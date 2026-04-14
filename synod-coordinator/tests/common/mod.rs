use synod_coordinator::config::Settings;
use sqlx::postgres::PgPoolOptions;
use synod_coordinator::AppState;
use redis::aio::ConnectionManager;
use serde::Serialize;
use tokio::net::TcpListener;
use uuid::Uuid;

pub async fn spawn_test_server() -> String {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();

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
            network_passphrase: "Test SDF Network ; September 2015".to_string(),
            horizon_url: "https://horizon-testnet.stellar.org".to_string(),
            coordinator_pubkey: "GD...".to_string(),
            coordinator_secret_key: "".to_string(),
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
    sqlx::query("DROP SCHEMA IF EXISTS public CASCADE").execute(&db_pool).await.unwrap();
    sqlx::query("CREATE SCHEMA IF NOT EXISTS public").execute(&db_pool).await.unwrap();

    // Run migrations so the schema is fresh
    sqlx::migrate!("./migrations")
        .run(&db_pool)
        .await
        .unwrap();

    let redis_client = redis::Client::open(settings.redis.url.as_str()).unwrap();
    let mut redis_client_conn = redis_client.get_connection().unwrap();
    redis::cmd("FLUSHALL").query::<()>(&mut redis_client_conn).unwrap();

    let redis_manager = ConnectionManager::new(redis_client).await.unwrap();

    let (tx_events, _) = tokio::sync::broadcast::channel(100);
    let state = AppState {
        db: db_pool,
        redis: redis_manager,
        config: settings,
        tx_events,
    };

    let app = synod_coordinator::router(state);
    
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{}", addr)
}

#[allow(dead_code)]
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

#[allow(dead_code)]
pub fn sign_with_key(signing_key: &ed25519_dalek::SigningKey, message: &[u8]) -> String {
    use ed25519_dalek::Signer;
    use sha2::{Sha256, Digest};
    // Replicate what Freighter does: SHA256("Stellar Signed Message:\n" + message)
    let mut hasher = Sha256::new();
    hasher.update(b"Stellar Signed Message:\n");
    hasher.update(message);
    let hashed = hasher.finalize();
    let signature = signing_key.sign(&hashed);
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, signature.to_bytes())
}
#[allow(dead_code)]
pub struct TestContext {
    pub base_url: String,
    pub client: reqwest::Client,
    pub db: sqlx::PgPool,
    pub user_token: String,
}

#[allow(dead_code)]
pub async fn setup_test_context() -> TestContext {
    let base_url = spawn_test_server().await;
    let client = reqwest::Client::new();
    
    // Connect to the same DB used by the test server (synod_db)
    let db_url = "postgres://postgres:postgres@localhost:5432/synod_db";
    let db = sqlx::PgPool::connect(db_url).await.unwrap();
    
    // 1. Register and Login
    let email = format!("test_{}@example.com", Uuid::new_v4());
    client.post(format!("{}/v1/auth/register", base_url))
        .json(&serde_json::json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();
    
    let login_resp = client.post(format!("{}/v1/auth/login", base_url))
        .json(&serde_json::json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();
    
    let login_data: serde_json::Value = login_resp.json().await.unwrap();
    let token = login_data["token"].as_str().unwrap().to_string();

    TestContext {
        base_url,
        client,
        db,
        user_token: token,
    }
}

#[allow(dead_code)]
pub async fn create_treasury(ctx: &TestContext, name: &str) -> Uuid {
    let response = ctx.client
        .post(format!("{}/v1/treasuries", ctx.base_url))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .json(&serde_json::json!({ "name": name, "network": "testnet" }))
        .send()
        .await
        .unwrap();

    let body: serde_json::Value = response.json().await.unwrap();
    Uuid::parse_str(body["treasury_id"].as_str().unwrap()).unwrap()
}

#[allow(dead_code)]
pub async fn attach_active_wallet(ctx: &TestContext, treasury_id: Uuid, wallet_address: &str) {
    sqlx::query(
        "INSERT INTO treasury_wallets (wallet_id, treasury_id, wallet_address, label, multisig_active, status, added_at)
         VALUES ($1, $2, $3, 'Test Wallet', false, 'ACTIVE', NOW())
         ON CONFLICT (treasury_id, wallet_address) DO UPDATE SET status = 'ACTIVE'"
    )
    .bind(Uuid::new_v4())
    .bind(treasury_id)
    .bind(wallet_address)
    .execute(&ctx.db)
    .await
    .unwrap();

    sqlx::query("UPDATE treasuries SET health = 'HEALTHY', updated_at = NOW() WHERE treasury_id = $1")
        .bind(treasury_id)
        .execute(&ctx.db)
        .await
        .unwrap();
}

#[allow(dead_code)]
pub async fn create_agent_slot(ctx: &TestContext, treasury_id: Uuid, name: &str) -> Uuid {
    let response = ctx.client
        .post(format!("{}/v1/agents/{}", ctx.base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .json(&serde_json::json!({ "name": name, "description": "Integration test agent" }))
        .send()
        .await
        .unwrap();

    let body: serde_json::Value = response.json().await.unwrap();
    Uuid::parse_str(body["agent"]["agent_id"].as_str().unwrap()).unwrap()
}

#[allow(dead_code)]
pub async fn enroll_agent_pubkey(
    ctx: &TestContext,
    agent_id: Uuid,
    wallet_address: &str,
    wallet_signing_key: &ed25519_dalek::SigningKey,
    agent_pubkey: &str,
) -> serde_json::Value {
    let challenge_response = ctx.client
        .post(format!("{}/v1/agents/{}/enroll/challenge", ctx.base_url, agent_id))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .json(&serde_json::json!({
            "wallet_address": wallet_address,
            "agent_pubkey": agent_pubkey,
        }))
        .send()
        .await
        .unwrap();

    let challenge_body: serde_json::Value = challenge_response.json().await.unwrap();
    let challenge = challenge_body["challenge"].as_str().unwrap();
    let message = format!(
        "synod-enroll:{}:{}:{}:{}",
        agent_id,
        wallet_address,
        agent_pubkey,
        challenge
    );
    let signature = sign_with_key(wallet_signing_key, message.as_bytes());

    let enroll_response = ctx.client
        .post(format!("{}/v1/agents/{}/enroll-pubkey", ctx.base_url, agent_id))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .json(&serde_json::json!({
            "wallet_address": wallet_address,
            "agent_pubkey": agent_pubkey,
            "challenge": challenge,
            "signature": signature,
        }))
        .send()
        .await
        .unwrap();

    enroll_response.json().await.unwrap()
}

#[allow(dead_code)]
pub async fn connect_agent(
    ctx: &TestContext,
    agent_pubkey: &str,
    agent_signing_key: &ed25519_dalek::SigningKey,
) -> serde_json::Value {
    let challenge_response = ctx.client
        .post(format!("{}/v1/agents/connect/challenge", ctx.base_url))
        .json(&serde_json::json!({ "agent_pubkey": agent_pubkey }))
        .send()
        .await
        .unwrap();

    let challenge_body: serde_json::Value = challenge_response.json().await.unwrap();
    let agent_id = challenge_body["agent_id"].as_str().unwrap();
    let treasury_id = challenge_body["treasury_id"].as_str().unwrap();
    let challenge = challenge_body["challenge"].as_str().unwrap();
    let message = format!(
        "synod-connect:{}:{}:{}:{}",
        agent_id,
        treasury_id,
        agent_pubkey,
        challenge
    );
    let signature = sign_with_key(agent_signing_key, message.as_bytes());

    let connect_response = ctx.client
        .post(format!("{}/v1/agents/connect/complete", ctx.base_url))
        .json(&serde_json::json!({
            "agent_pubkey": agent_pubkey,
            "challenge": challenge,
            "signature": signature,
        }))
        .send()
        .await
        .unwrap();

    connect_response.json().await.unwrap()
}

#[allow(dead_code)]
pub fn build_signed_request_auth<T: Serialize>(
    signing_key: &ed25519_dalek::SigningKey,
    agent_pubkey: &str,
    agent_id: Uuid,
    op_name: &str,
    payload: &T,
) -> serde_json::Value {
    let request_id = Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().timestamp();
    let payload_json = serde_json::to_string(payload).unwrap();
    let message = format!(
        "synod-request:{}:{}:{}:{}:{}",
        op_name,
        agent_id,
        request_id,
        timestamp,
        payload_json
    );

    serde_json::json!({
        "agent_pubkey": agent_pubkey,
        "request_id": request_id,
        "timestamp": timestamp,
        "signature": sign_with_key(signing_key, message.as_bytes()),
    })
}
