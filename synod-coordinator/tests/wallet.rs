use axum::http::StatusCode;
use serde_json::json;
mod common;
use common::{generate_test_stellar_keypair, sign_with_key, spawn_test_server};

#[serial_test::serial]
#[tokio::test]
async fn test_phase_3_wallet_flow() {
    let base_url = spawn_test_server().await;
    let client = reqwest::Client::new();

    // 1. Register User & Login to get JWT
    let email = format!("wallet_{}@test.com", uuid::Uuid::new_v4());
    let register_res = client
        .post(format!("{}/v1/auth/register", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send()
        .await
        .unwrap();
    let reg_status = register_res.status();
    let reg_body = register_res.text().await.unwrap();
    println!("Register response: {} - {}", reg_status, reg_body);
    assert_eq!(reg_status, StatusCode::OK);

    let login_res = client
        .post(format!("{}/v1/auth/login", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send()
        .await
        .unwrap();
    let login_status = login_res.status();
    let login_text = login_res.text().await.unwrap();
    println!("Login response: {} - {}", login_status, login_text);

    let login_body: serde_json::Value = serde_json::from_str(&login_text).unwrap();
    let token = login_body["token"]
        .as_str()
        .expect("Token missing in login response");

    // 2. Create Treasury
    let treasury_res = client
        .post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "name": "Test Treasury", "network": "testnet" }))
        .send()
        .await
        .unwrap();
    assert_eq!(treasury_res.status(), StatusCode::OK);
    let treasury_body: serde_json::Value = treasury_res.json().await.unwrap();
    let treasury_id = treasury_body["treasury_id"].as_str().unwrap();

    // 3. Generate Stellar keypair for ownership
    let (signing_key, pk_stellar) = generate_test_stellar_keypair();

    // 4. Register Wallet using generated address
    let reg_wallet_res = client
        .post(format!(
            "{}/v1/treasuries/{}/wallets",
            base_url, treasury_id
        ))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "wallet_address": pk_stellar, "label": "Main Wallet" }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg_wallet_res.status(), StatusCode::CREATED);

    // 5. Get Nonce for our generated wallet address
    let nonce_res = client
        .post(format!("{}/v1/wallets/nonce", base_url))
        .json(&json!({ "wallet_address": pk_stellar }))
        .send()
        .await
        .unwrap();
    assert_eq!(nonce_res.status(), StatusCode::OK);
    let nonce_body: serde_json::Value = nonce_res.json().await.unwrap();
    let nonce = nonce_body["nonce"].as_str().unwrap();

    // 6. Sign the nonce with our Ed25519 key and verify ownership
    let sig_base64 = sign_with_key(&signing_key, nonce.as_bytes());

    let verify_res = client
        .post(format!("{}/v1/wallets/verify-ownership", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "wallet_address": pk_stellar,
            "nonce": nonce,
            "signature": sig_base64
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(verify_res.status(), StatusCode::OK);

    // 7. Connect Wallet (WalletConnect Topic)
    let connect_res = client
        .post(format!("{}/v1/wallets/connect", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "wallet_address": pk_stellar,
            "wc_session_topic": "topic_123",
            "wc_session_expiry": "2030-01-01T00:00:00Z"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(connect_res.status(), StatusCode::OK);
}
