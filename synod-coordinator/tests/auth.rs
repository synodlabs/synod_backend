use axum::{body::Body, extract::Request, http::{header, Method, StatusCode}};
use reqwest;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use synod_coordinator::{config::Settings, AppState};
use std::time::Duration;
use tokio::net::TcpListener;
use axum_extra::headers::authorization::Bearer;
use axum_extra::headers::Authorization;

async fn spawn_test_server() -> String {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("debug,synod_coordinator=trace")
        .try_init();

    let settings = Settings {
        server: synod_coordinator::config::ServerConfig::default(),
        database: synod_coordinator::config::DatabaseConfig {
            url: "postgres://postgres:postgres@localhost:5432/synod_db".to_string(),
            max_connections: 5,
        },
        redis: synod_coordinator::config::RedisConfig {
            url: "redis://localhost:6379".to_string(),
        },
        stellar: synod_coordinator::config::StellarConfig {
            network: "testnet".to_string(),
            horizon_url: "".to_string(),
            coordinator_pubkey: "".to_string(),
            coordinator_secret_key_path: "".to_string(),
        },
        auth: synod_coordinator::config::AuthConfig {
            jwt_secret: "test_secret_key_very_long_for_hmac".to_string(),
            jwt_expiry_hours: 1,
            bcrypt_cost: 4, // Fast cost for tests
        },
        walletconnect: synod_coordinator::config::WalletConnectConfig {
            project_id: "".to_string(),
            relay_url: "".to_string(),
        },
    };

    let db_pool = PgPoolOptions::new()
        .connect(&settings.database.url)
        .await
        .expect("Failed to connect to testing DB");

    sqlx::migrate!("./migrations")
        .run(&db_pool)
        .await
        .expect("Failed to run migrations");

    // Clean tables for tests
    sqlx::query("TRUNCATE TABLE users CASCADE").execute(&db_pool).await.unwrap();

    let redis_client = redis::Client::open(settings.redis.url.as_str()).unwrap();
    let mut redis_manager = redis::aio::ConnectionManager::new(redis_client).await.unwrap();
    
    // Clear Redis rate limits
    redis::cmd("FLUSHALL").query_async::<_, ()>(&mut redis_manager).await.unwrap();

    let state = AppState {
        db: db_pool,
        redis: redis_manager,
        config: settings,
    };

    // Spin up an actual local server on random port
    let app = axum::Router::new()
        .nest("/v1/auth", synod_coordinator::auth::router())
        .with_state(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    format!("http://127.0.0.1:{}", port)
}

#[tokio::test]
async fn test_email_password_flow() {
    let base_url = spawn_test_server().await;
    let client = reqwest::Client::new();

    let email = "test@example.com";
    let password = "super_secret_password";

    // 1. Register
    let reg_res = client.post(&format!("{}/v1/auth/register", base_url))
        .json(&json!({ "email": email, "password": password }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg_res.status(), StatusCode::OK);
    
    let reg_body: Value = reg_res.json().await.unwrap();
    assert!(reg_body.get("token").is_some(), "Registration should return a JWT");
    
    // 2. Login
    let login_res = client.post(&format!("{}/v1/auth/login", base_url))
        .json(&json!({ "email": email, "password": password }))
        .send()
        .await
        .unwrap();
    assert_eq!(login_res.status(), StatusCode::OK);
    let login_body: Value = login_res.json().await.unwrap();
    assert!(login_body.get("token").is_some(), "Login should return a JWT");
}

#[tokio::test]
async fn test_rate_limiting_11th_attempt_fails() {
    let base_url = spawn_test_server().await;
    let client = reqwest::Client::new();
    let email = "rate_limit@example.com";

    // Rate limit prefix triggers on login, not register
    for i in 1..=11 {
        let res = client.post(&format!("{}/v1/auth/login", base_url))
            .json(&json!({ "email": email, "password": "wrongpassword" }))
            .send()
            .await
            .unwrap();

        if i <= 10 {
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        } else {
            assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        }
    }
}

#[tokio::test]
async fn test_passkey_flow() {
    let base_url = spawn_test_server().await;
    let client = reqwest::Client::new();
    let email = "passkey_user@example.com";

    // 1. Register Begin
    let begin_res = client.post(&format!("{}/v1/auth/passkey/register/begin", base_url))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap();
    assert_eq!(begin_res.status(), StatusCode::OK);
    let begin_body: Value = begin_res.json().await.unwrap();
    let challenge = begin_body.get("challenge").unwrap().as_str().unwrap();

    // 2. Register Complete
    let complete_res = client.post(&format!("{}/v1/auth/passkey/register/complete", base_url))
        .json(&json!({ "email": email, "challenge": challenge, "credential_id": "mock_cred" }))
        .send()
        .await
        .unwrap();
    assert_eq!(complete_res.status(), StatusCode::OK);
    let complete_body: Value = complete_res.json().await.unwrap();
    assert!(complete_body.get("token").is_some());

    // 3. Login Begin
    let login_begin_res = client.post(&format!("{}/v1/auth/passkey/login/begin", base_url))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap();
    assert_eq!(login_begin_res.status(), StatusCode::OK);
    let login_begin_body: Value = login_begin_res.json().await.unwrap();
    let login_challenge = login_begin_body.get("challenge").unwrap().as_str().unwrap();

    // 4. Login Complete
    let login_complete_res = client.post(&format!("{}/v1/auth/passkey/login/complete", base_url))
        .json(&json!({ "email": email, "challenge": login_challenge, "credential_id": "mock_cred" }))
        .send()
        .await
        .unwrap();
    assert_eq!(login_complete_res.status(), StatusCode::OK);
    let login_complete_body: Value = login_complete_res.json().await.unwrap();
    assert!(login_complete_body.get("token").is_some());
}
