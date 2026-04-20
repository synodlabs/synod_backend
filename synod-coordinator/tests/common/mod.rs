use redis::aio::ConnectionManager;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use std::{collections::HashMap, sync::Arc};
use synod_coordinator::config::Settings;
use synod_coordinator::{AppState, WatcherHandles};
use tokio::net::TcpListener;
use tokio::{sync::Mutex, task::JoinHandle};
use uuid::Uuid;

pub async fn spawn_test_server() -> String {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_test_writer()
        .try_init();

    let (coordinator_signing_key, coordinator_pubkey) = generate_test_stellar_keypair();
    let coordinator_secret_key = encode_test_stellar_secret(&coordinator_signing_key);

    let settings = Settings {
        server: synod_coordinator::config::ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
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
            horizon_url: std::env::var("SYNOD_TEST_HORIZON_URL")
                .unwrap_or_else(|_| "https://horizon-testnet.stellar.org".to_string()),
            coordinator_pubkey,
            coordinator_secret_key,
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
    sqlx::query("DROP SCHEMA IF EXISTS public CASCADE")
        .execute(&db_pool)
        .await
        .unwrap();
    sqlx::query("CREATE SCHEMA IF NOT EXISTS public")
        .execute(&db_pool)
        .await
        .unwrap();

    // Run migrations so the schema is fresh
    sqlx::migrate!("./migrations").run(&db_pool).await.unwrap();

    let redis_client = redis::Client::open(settings.redis.url.as_str()).unwrap();
    let mut redis_client_conn = redis_client.get_connection().unwrap();
    redis::cmd("FLUSHALL")
        .query::<()>(&mut redis_client_conn)
        .unwrap();

    let redis_manager = ConnectionManager::new(redis_client).await.unwrap();

    let (tx_events, _) = tokio::sync::broadcast::channel(100);
    let state = AppState {
        db: db_pool,
        redis: redis_manager,
        config: settings,
        tx_events,
        watcher_handles: Arc::new(Mutex::new(HashMap::<String, JoinHandle<()>>::new()))
            as WatcherHandles,
    };

    let db_pool_events = state.db.clone();
    let mut rx_event_store = state.tx_events.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = rx_event_store.recv().await {
            let _ = synod_coordinator::persist_treasury_event(&db_pool_events, &event).await;
        }
    });

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
pub fn encode_test_stellar_secret(signing_key: &ed25519_dalek::SigningKey) -> String {
    let mut raw = vec![0x90u8];
    raw.extend_from_slice(&signing_key.to_bytes());
    raw.extend_from_slice(&[0, 0]);

    data_encoding::BASE32_NOPAD.encode(&raw)
}

#[allow(dead_code)]
pub fn build_test_payment_envelope_xdr(
    source_address: &str,
    destination_address: &str,
    amount_stroops: i64,
) -> String {
    use synod_coordinator::stellar::next_xdr::{
        Asset, DecoratedSignature, Limits, Memo, MuxedAccount, Operation, OperationBody, PaymentOp,
        Preconditions, SequenceNumber, Transaction, TransactionEnvelope, TransactionExt,
        TransactionV1Envelope, Uint256, WriteXdr,
    };

    let source = MuxedAccount::Ed25519(Uint256(
        synod_coordinator::stellar::decode_stellar_address(source_address).unwrap(),
    ));
    let destination = MuxedAccount::Ed25519(Uint256(
        synod_coordinator::stellar::decode_stellar_address(destination_address).unwrap(),
    ));

    let operation = Operation {
        source_account: None,
        body: OperationBody::Payment(PaymentOp {
            destination,
            asset: Asset::Native,
            amount: amount_stroops,
        }),
    };

    let transaction = Transaction {
        source_account: source,
        fee: 100,
        seq_num: SequenceNumber(1),
        cond: Preconditions::None,
        memo: Memo::None,
        operations: vec![operation].try_into().unwrap(),
        ext: TransactionExt::V0,
    };

    let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
        tx: transaction,
        signatures: Vec::<DecoratedSignature>::new().try_into().unwrap(),
    });

    let raw = envelope.to_xdr(Limits::none()).unwrap();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw)
}

#[allow(dead_code)]
pub fn build_signed_test_payment_envelope_xdr(
    source_address: &str,
    destination_address: &str,
    amount_stroops: i64,
    signing_key: &ed25519_dalek::SigningKey,
) -> String {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use ed25519_dalek::Signer;
    use synod_coordinator::stellar::next_xdr::{
        DecoratedSignature, Limits, ReadXdr, Signature, SignatureHint, TransactionEnvelope, WriteXdr,
    };

    let xdr = build_test_payment_envelope_xdr(source_address, destination_address, amount_stroops);
    let raw_env = BASE64.decode(&xdr).unwrap();
    let mut envelope = TransactionEnvelope::from_xdr(&raw_env, Limits::none()).unwrap();
    let candidate_hashes = synod_coordinator::stellar::calculate_tx_v1_hashes(
        &raw_env,
        "Test SDF Network ; September 2015",
    )
    .unwrap();
    let hash = candidate_hashes[0];
    let signature_bytes = signing_key.sign(&hash).to_bytes();

    let TransactionEnvelope::Tx(v1_env) = &mut envelope else {
        panic!("expected v1 envelope");
    };

    let pubkey_bytes = signing_key.verifying_key().to_bytes();
    let hint = [
        pubkey_bytes[28],
        pubkey_bytes[29],
        pubkey_bytes[30],
        pubkey_bytes[31],
    ];

    let mut signatures: Vec<DecoratedSignature> = v1_env.signatures.clone().into();
    signatures.push(DecoratedSignature {
        hint: SignatureHint(hint),
        signature: Signature(signature_bytes.to_vec().try_into().unwrap()),
    });
    v1_env.signatures = signatures.try_into().unwrap();

    let final_xdr = envelope.to_xdr(Limits::none()).unwrap();
    BASE64.encode(final_xdr)
}

#[allow(dead_code)]
pub async fn spawn_mock_horizon_server(tx_hash: &str) -> String {
    use axum::{routing::post, Json, Router};
    use tokio::net::TcpListener;

    async fn submit_transaction(
        axum::extract::State(hash): axum::extract::State<String>,
    ) -> Json<serde_json::Value> {
        Json(serde_json::json!({ "hash": hash }))
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/transactions", post(submit_transaction))
        .with_state(tx_hash.to_string());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://{}", addr)
}

#[allow(dead_code)]
pub fn sign_with_key(signing_key: &ed25519_dalek::SigningKey, message: &[u8]) -> String {
    use ed25519_dalek::Signer;
    use sha2::{Digest, Sha256};
    // Replicate what Freighter does: SHA256("Stellar Signed Message:\n" + message)
    let mut hasher = Sha256::new();
    hasher.update(b"Stellar Signed Message:\n");
    hasher.update(message);
    let hashed = hasher.finalize();
    let signature = signing_key.sign(&hashed);
    base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        signature.to_bytes(),
    )
}

#[allow(dead_code)]
pub fn sign_raw_bytes(signing_key: &ed25519_dalek::SigningKey, message: &[u8]) -> String {
    use ed25519_dalek::Signer;

    let signature = signing_key.sign(message);
    base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        signature.to_bytes(),
    )
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
    client
        .post(format!("{}/v1/auth/register", base_url))
        .json(&serde_json::json!({ "email": email, "password": "password123" }))
        .send()
        .await
        .unwrap();

    let login_resp = client
        .post(format!("{}/v1/auth/login", base_url))
        .json(&serde_json::json!({ "email": email, "password": "password123" }))
        .send()
        .await
        .unwrap();

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
    let response = ctx
        .client
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

    sqlx::query(
        "UPDATE treasuries SET health = 'HEALTHY', updated_at = NOW() WHERE treasury_id = $1",
    )
    .bind(treasury_id)
    .execute(&ctx.db)
    .await
    .unwrap();
}

#[allow(dead_code)]
pub async fn create_agent_slot(
    ctx: &TestContext,
    treasury_id: Uuid,
    name: &str,
    agent_pubkey: &str,
) -> Uuid {
    let response = ctx
        .client
        .post(format!("{}/v1/agents/{}", ctx.base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .json(&serde_json::json!({
            "name": name,
            "description": "Integration test agent",
            "agent_pubkey": agent_pubkey,
        }))
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
    let challenge_response = ctx
        .client
        .post(format!(
            "{}/v1/agents/{}/enroll/challenge",
            ctx.base_url, agent_id
        ))
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
        agent_id, wallet_address, agent_pubkey, challenge
    );
    let signature = sign_with_key(wallet_signing_key, message.as_bytes());

    let enroll_response = ctx
        .client
        .post(format!(
            "{}/v1/agents/{}/enroll-pubkey",
            ctx.base_url, agent_id
        ))
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
    let challenge_response = ctx
        .client
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
        agent_id, treasury_id, agent_pubkey, challenge
    );
    let signature = sign_with_key(agent_signing_key, message.as_bytes());

    let connect_response = ctx
        .client
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
pub async fn connect_agent_mcp(
    ctx: &TestContext,
    agent_pubkey: &str,
    agent_signing_key: &ed25519_dalek::SigningKey,
) -> serde_json::Value {
    let init_response = ctx
        .client
        .post(format!("{}/connect/init", ctx.base_url))
        .json(&serde_json::json!({ "public_key": agent_pubkey }))
        .send()
        .await
        .unwrap();

    let init_body: serde_json::Value = init_response.json().await.unwrap();
    let nonce = init_body["nonce"].as_str().unwrap();
    let payload = format!(
        r#"{{"action":"connect","domain":"synod","nonce":"{}"}}"#,
        nonce
    );
    let hash = synod_coordinator::stellar::sha256_bytes(payload.as_bytes());
    let signature = sign_raw_bytes(agent_signing_key, &hash);

    let complete_response = ctx
        .client
        .post(format!("{}/connect/complete", ctx.base_url))
        .json(&serde_json::json!({
            "public_key": agent_pubkey,
            "signature": signature,
            "nonce": nonce,
        }))
        .send()
        .await
        .unwrap();

    complete_response.json().await.unwrap()
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
        op_name, agent_id, request_id, timestamp, payload_json
    );

    serde_json::json!({
        "agent_pubkey": agent_pubkey,
        "request_id": request_id,
        "timestamp": timestamp,
        "signature": sign_with_key(signing_key, message.as_bytes()),
    })
}
