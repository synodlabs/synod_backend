use axum::http::StatusCode;
use reqwest::Client;
use serde_json::json;
use sha2::Digest;
use uuid::Uuid;

mod common;
use common::{spawn_test_server, generate_test_stellar_keypair, sign_with_key};
use synod_coordinator::constitution::{PoolConfig, ConstitutionContent};

#[tokio::test]
async fn test_phase_5_constitution_flow() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    // 1. Setup: Register, Login, Create Treasury
    let email = format!("governance_{}@test.com", Uuid::new_v4());
    client.post(format!("{}/v1/auth/register", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();
        
    let login_res = client.post(format!("{}/v1/auth/login", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();
    let login_body: serde_json::Value = login_res.json().await.unwrap();
    let token = login_body["token"].as_str().unwrap();

    let treasury_res = client.post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "name": "Gov Treasury", "network": "testnet" }))
        .send().await.unwrap();
    let treasury_body: serde_json::Value = treasury_res.json().await.unwrap();
    let treasury_id = treasury_body["treasury_id"].as_str().unwrap();

    // 2. Test Constitution Creation (Admin Override flow used in CRUD)
    let valid_constitution = json!({
        "content": {
            "pools": [
                {
                    "pool_key": "reserves_pool",
                    "asset_code": "USDC",
                    "target_pct": 50.0,
                    "floor_pct": 40.0,
                    "ceiling_pct": 60.0,
                    "drift_bounds_pct": 2.0
                },
                {
                    "pool_key": "growth_pool",
                    "asset_code": "XLM",
                    "target_pct": 50.0,
                    "floor_pct": 30.0,
                    "ceiling_pct": 70.0,
                    "drift_bounds_pct": 5.0
                }
            ]
        }
    });

    let create_res = client.post(format!("{}/v1/treasuries/{}/constitution", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&valid_constitution)
        .send().await.unwrap();
    
    let status = create_res.status();
    let create_body: serde_json::Value = create_res.json().await.unwrap();
    assert_eq!(status, StatusCode::CREATED, "Failed to create constitution: {:?}", create_body);
    assert_eq!(create_body["version"].as_i64().unwrap(), 1);

    // 3. Test Invalid Constitution (Math failure)
    let invalid_constitution = json!({
        "content": {
            "pools": [
                {
                    "pool_key": "reserves_pool",
                    "asset_code": "USDC",
                    "target_pct": 90.0, // Sum = 140.0
                    "floor_pct": 40.0,
                    "ceiling_pct": 100.0,
                    "drift_bounds_pct": 2.0
                },
                {
                    "pool_key": "growth_pool",
                    "asset_code": "XLM",
                    "target_pct": 50.0,
                    "floor_pct": 30.0,
                    "ceiling_pct": 70.0,
                    "drift_bounds_pct": 5.0
                }
            ]
        }
    });

    let invalid_res = client.post(format!("{}/v1/treasuries/{}/constitution", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&invalid_constitution)
        .send().await.unwrap();
    
    assert_eq!(invalid_res.status(), StatusCode::BAD_REQUEST);

    // 4. Test Proposal Creation
    let proposal_req = json!({
        "content": {
            "pools": [
                 {
                    "pool_key": "single_asset",
                    "asset_code": "USDC",
                    "target_pct": 100.0,
                    "floor_pct": 80.0,
                    "ceiling_pct": 110.0,
                    "drift_bounds_pct": 5.0
                }
            ]
        }
    });

    let prop_res = client.post(format!("{}/v1/treasuries/{}/proposals", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&proposal_req)
        .send().await.unwrap();

    let prop_status = prop_res.status();
    let prop_body: serde_json::Value = prop_res.json().await.unwrap();
    assert_eq!(prop_status, StatusCode::CREATED, "Failed to create proposal: {:?}", prop_body);
    let _proposal_id = prop_body["proposal_id"].as_str().unwrap();

    // 5. Test Rollback to V1
    let rollback_res = client.post(format!("{}/v1/treasuries/{}/constitution/rollback/1", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .send().await.unwrap();
    
    let rb_status = rollback_res.status();
    let rb_body: serde_json::Value = rollback_res.json().await.unwrap();
    assert_eq!(rb_status, StatusCode::CREATED, "Failed to rollback: {:?}", rb_body);
    assert_eq!(rb_body["version"].as_i64().unwrap(), 2);
    
    let state_hash_v1 = create_body["state_hash"].as_str().unwrap();
    let state_hash_v2 = rb_body["state_hash"].as_str().unwrap();
    assert_eq!(state_hash_v1, state_hash_v2); // State matches exactly!
}

#[tokio::test]
async fn test_phase_5_governance_signing() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    // 1. Setup: Register, Login, Create Treasury
    let email = format!("signing_{}@test.com", Uuid::new_v4());
    client.post(format!("{}/v1/auth/register", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();
        
    let login_res = client.post(format!("{}/v1/auth/login", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();
    let login_body: serde_json::Value = login_res.json().await.unwrap();
    let token = login_body["token"].as_str().unwrap();

    let treasury_res = client.post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "name": "Multi-sig Treasury", "network": "testnet" }))
        .send().await.unwrap();
    let treasury_body: serde_json::Value = treasury_res.json().await.unwrap();
    let treasury_id = treasury_body["treasury_id"].as_str().unwrap();

    // 2. Register and Verify a Wallet (so it becomes ACTIVE)
    let (signing_key, wallet_address) = generate_test_stellar_keypair();
    client.post(format!("{}/v1/treasuries/{}/wallets", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "wallet_address": wallet_address, "label": "Signer 1" }))
        .send().await.unwrap();

    let nonce_res = client.post(format!("{}/v1/wallets/nonce", base_url))
        .json(&json!({ "wallet_address": wallet_address }))
        .send().await.unwrap();
    let nonce_body: serde_json::Value = nonce_res.json().await.unwrap();
    let nonce = nonce_body["nonce"].as_str().unwrap().to_string();
    let sig_base64 = sign_with_key(&signing_key, nonce.as_bytes());

    client.post(format!("{}/v1/wallets/verify-ownership", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "wallet_address": wallet_address, "nonce": nonce, "signature": sig_base64 }))
        .send().await.unwrap();

    // 3. Create a Version 1 Constitution (Base)
    let v1_content = json!({
        "pools": [
            {
                "pool_key": "main",
                "asset_code": "USDC",
                "target_pct": 100.0,
                "floor_pct": 80.0,
                "ceiling_pct": 110.0,
                "drift_bounds_pct": 5.0
            }
        ]
    });
    client.post(format!("{}/v1/treasuries/{}/constitution", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "content": v1_content }))
        .send().await.unwrap();

    // 4. Propose a Change (Version 2)
    let v2_content = ConstitutionContent {
        memo: Some("test change".to_string()),
        pools: vec![
            PoolConfig {
                pool_key: "main".to_string(),
                asset_code: "USDC".to_string(),
                target_pct: 100.0,
                floor_pct: 80.0,
                ceiling_pct: 115.0,
                drift_bounds_pct: 5.0,
            }
        ],
    };

    let prop_res = client.post(format!("{}/v1/treasuries/{}/proposals", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "content": v2_content }))
        .send().await.unwrap();
    let prop_body: serde_json::Value = prop_res.json().await.unwrap();
    let proposal_id = prop_body["proposal_id"].as_str().unwrap().to_string();

    // 5. Sign the Proposal
    // Use the backend's hashing logic by using the same struct serialization
    let json_bytes = serde_json::to_vec(&v2_content).unwrap();
    let hash = sha2::Sha256::digest(&json_bytes);
    let hash_hex = hex::encode(hash);
    
    // The backend expects the signature of "SYNOD_PROPOSAL:{hash_hex}"
    let msg = format!("SYNOD_PROPOSAL:{}", hash_hex);
    let sig_prop = sign_with_key(&signing_key, msg.as_bytes());

    let sign_res = client.post(format!("{}/v1/treasuries/{}/proposals/{}/sign", base_url, treasury_id, proposal_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "wallet_address": wallet_address, "signature_base64": sig_prop }))
        .send().await.unwrap();
    
    let sign_status = sign_res.status();
    let sign_text = sign_res.text().await.unwrap();
    if sign_status != StatusCode::OK {
        panic!("Failed to sign proposal: {} - {}", sign_status, sign_text);
    }
    let sign_body: serde_json::Value = serde_json::from_str(&sign_text).unwrap();
    assert_eq!(sign_body["status"].as_str().unwrap(), "EXECUTED");

    // 6. Verify Constitution history now has version 2
    let history_res = client.get(format!("{}/v1/treasuries/{}/constitution/history", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .send().await.unwrap();
    let history: Vec<serde_json::Value> = history_res.json().await.unwrap();
    
    // History should have [v2, v1] (ordered by version DESC)
    assert_eq!(history.len(), 2);
    assert_eq!(history[0]["version"].as_i64().unwrap(), 2);
    assert_eq!(history[1]["version"].as_i64().unwrap(), 1);
}
