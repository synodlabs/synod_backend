use axum::http::StatusCode;
use serde_json::json;
use synod_coordinator::config::Settings;
use sqlx::postgres::PgPoolOptions;
use synod_coordinator::AppState;
use redis::aio::ConnectionManager;
use tokio::net::TcpListener;

async fn spawn_test_server() -> String {
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

    let redis_client = redis::Client::open(settings.redis.url.as_str()).unwrap();
    let redis_manager = ConnectionManager::new(redis_client).await.unwrap();

    let state = AppState {
        db: db_pool,
        redis: redis_manager,
        config: settings,
    };

    let app = synod_coordinator::router(state); // We need to add a router helper or use main's logic
    
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{}", addr)
}

#[tokio::test]
async fn test_phase_3_wallet_flow() {
    let base_url = spawn_test_server().await;
    let client = reqwest::Client::new();

    // 1. Register User & Login to get JWT
    let email = format!("wallet_{}@test.com", uuid::Uuid::new_v4());
    let register_res = client.post(format!("{}/v1/auth/register", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();
    assert_eq!(register_res.status(), StatusCode::OK);
    
    let login_res = client.post(format!("{}/v1/auth/login", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();
    let login_body: serde_json::Value = login_res.json().await.unwrap();
    let token = login_body["token"].as_str().unwrap();

    // 2. Create Treasury
    let treasury_res = client.post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "name": "Test Treasury", "network": "testnet" }))
        .send().await.unwrap();
    assert_eq!(treasury_res.status(), StatusCode::OK);
    let treasury_body: serde_json::Value = treasury_res.json().await.unwrap();
    let treasury_id = treasury_body["treasury_id"].as_str().unwrap();

    // 3. Generate Stellar keypair for ownership
    let (signing_key, pk_stellar) = generate_test_stellar_keypair();

    // 4. Register Wallet using generated address
    let reg_wallet_res = client.post(format!("{}/v1/treasuries/{}/wallets", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "wallet_address": pk_stellar, "label": "Main Wallet" }))
        .send().await.unwrap();
    assert_eq!(reg_wallet_res.status(), StatusCode::CREATED);

    // 5. Get Nonce for our generated wallet address
    let nonce_res = client.post(format!("{}/v1/wallets/nonce", base_url))
        .json(&json!({ "wallet_address": pk_stellar }))
        .send().await.unwrap();
    assert_eq!(nonce_res.status(), StatusCode::OK);
    let nonce_body: serde_json::Value = nonce_res.json().await.unwrap();
    let nonce = nonce_body["nonce"].as_str().unwrap();

    // 6. Sign the nonce with our Ed25519 key and verify ownership
    let sig_base64 = sign_with_key(&signing_key, nonce.as_bytes());

    let verify_res = client.post(format!("{}/v1/wallets/verify-ownership", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "wallet_address": pk_stellar, 
            "nonce": nonce,
            "signature": sig_base64
        }))
        .send().await.unwrap();
    assert_eq!(verify_res.status(), StatusCode::OK);

    // 7. Connect Wallet (WalletConnect Topic)
    let connect_res = client.post(format!("{}/v1/wallets/connect", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "wallet_address": pk_stellar,
            "wc_session_topic": "topic_123",
            "wc_session_expiry": "2030-01-01T00:00:00Z"
        }))
        .send().await.unwrap();
    assert_eq!(connect_res.status(), StatusCode::OK);
}

fn generate_test_stellar_keypair() -> (ed25519_dalek::SigningKey, String) {
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;
    
    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    
    // Encode as Stellar G... address: [version_byte(0x30), pk(32), crc(2)]
    let mut raw = vec![0x30u8];
    raw.extend_from_slice(&verifying_key.to_bytes());
    raw.extend_from_slice(&[0, 0]); // Mock CRC (our decoder ignores it)
    
    let pk_stellar = data_encoding::BASE32_NOPAD.encode(&raw);
    (signing_key, pk_stellar)
}

fn sign_with_key(signing_key: &ed25519_dalek::SigningKey, message: &[u8]) -> String {
    use ed25519_dalek::Signer;
    let signature = signing_key.sign(message);
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, signature.to_bytes())
}
