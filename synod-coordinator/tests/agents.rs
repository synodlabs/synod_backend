use axum::http::StatusCode;
use reqwest::Client;
use serde_json::json;
use uuid::Uuid;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures::{SinkExt, StreamExt};

mod common;
use common::spawn_test_server;

#[tokio::test]
async fn test_phase_6_agent_lifecycle() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    // 1. Setup: Register, Login, Create Treasury
    let email = format!("agent_dev_{}@test.com", Uuid::new_v4());
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
        .json(&json!({ "name": "Agent Treasury", "network": "testnet" }))
        .send().await.unwrap();
    let treasury_body: serde_json::Value = treasury_res.json().await.unwrap();
    let treasury_id = treasury_body["treasury_id"].as_str().unwrap();

    // 2. Create Agent Slot
    let agent_res = client.post(format!("{}/v1/agents/{}", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "name": "Alpha Agent", "description": "High frequency trader" }))
        .send().await.unwrap();
    
    assert_eq!(agent_res.status(), StatusCode::CREATED);
    let agent_body: serde_json::Value = agent_res.json().await.unwrap();
    let agent_id = agent_body["agent"]["agent_id"].as_str().unwrap();
    let api_key = agent_body["api_key"].as_str().unwrap();
    
    assert_eq!(agent_body["agent"]["status"].as_str().unwrap(), "PENDING_CONNECTION");

    // 3. Handshake (Connect)
    let agent_pubkey = "GDJJ7Z...MOCK"; // Mock Stellar address
    let handshake_res = client.post(format!("{}/v1/agents/handshake", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": agent_pubkey
        }))
        .send().await.unwrap();
    
    assert_eq!(handshake_res.status(), StatusCode::OK);
    let handshake_body: serde_json::Value = handshake_res.json().await.unwrap();
    assert_eq!(handshake_body["status"].as_str().unwrap(), "ACTIVE");
    assert_eq!(handshake_body["agent_pubkey"].as_str().unwrap(), agent_pubkey);

    // 4. Test WebSocket Connectivity
    let ws_url = base_url.replace("http://", "ws://") + &format!("/v1/agents/ws/{}", agent_id);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("Failed to connect to WebSocket");

    // a) Ping / Pong
    ws_stream.send(Message::Text("ping".into())).await.unwrap();
    let msg = tokio::time::timeout(tokio::time::Duration::from_secs(2), ws_stream.next()).await
        .expect("Ping timed out")
        .expect("Stream closed prematurely")
        .expect("Error receiving pong");

    match msg {
        Message::Text(text) => assert_eq!(text, "pong"),
        _ => panic!("Expected text message, got {:?}", msg),
    }

    // b) Receive Real-time Event (Constitution Update)
    let trigger_res = client.post(format!("{}/v1/treasuries/{}", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "content": {
                "pools": [
                    { "pool_key": "pool:XLM", "asset_code": "XLM", "target_pct": 100.0, "floor_pct": 0.0, "ceiling_pct": 100.0, "drift_bounds_pct": 0.0 }
                ]
            }
        }))
        .send().await.unwrap();

    assert_eq!(trigger_res.status(), axum::http::StatusCode::CREATED, "Constitution update failed: {:?}", trigger_res.text().await.unwrap());

    // Check WS for event with timeout
    let event_msg = tokio::time::timeout(tokio::time::Duration::from_secs(5), ws_stream.next()).await
        .expect("ConstitutionUpdate timed out")
        .expect("Stream closed prematurely")
        .expect("Error receiving event");

    match event_msg {
        Message::Text(text) => {
            let event: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(event["type"].as_str().unwrap(), "CONSTITUTION_UPDATE");
            assert_eq!(event["version"].as_i64().unwrap(), 1);
        },
        _ => panic!("Expected text event message, got {:?}", event_msg),
    }

    // 5. Suspend Agent
    let suspend_res = client.post(format!("{}/v1/agents/{}/{}/suspend", base_url, treasury_id, agent_id))
        .header("Authorization", format!("Bearer {}", token))
        .send().await.unwrap();
    
    assert_eq!(suspend_res.status(), StatusCode::OK);
    let suspend_body: serde_json::Value = suspend_res.json().await.unwrap();
    assert_eq!(suspend_body["status"].as_str().unwrap(), "SUSPENDED");

    // Verify WebSocket disconnects or receives error (In this mock, we just check status)
    // In a full implementation, the heartbeat would fail.
}
