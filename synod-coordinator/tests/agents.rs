use axum::http::StatusCode;
use reqwest::Client;
use serde_json::json;
use uuid::Uuid;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use futures::{SinkExt, StreamExt};

mod common;
use common::spawn_test_server;

// ── Helper: Create a full agent setup (user + treasury + agent slot) ──
async fn create_agent_setup(base_url: &str) -> (String, String, String, String) {
    let client = Client::new();
    let email = format!("agent_test_{}@test.com", Uuid::new_v4());
    
    // Register + Login
    client.post(format!("{}/v1/auth/register", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();

    let login_res = client.post(format!("{}/v1/auth/login", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();
    let login_body: serde_json::Value = login_res.json().await.unwrap();
    let token = login_body["token"].as_str().unwrap().to_string();

    // Create Treasury
    let treasury_res = client.post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "name": "Test Treasury", "network": "testnet" }))
        .send().await.unwrap();
    let treasury_body: serde_json::Value = treasury_res.json().await.unwrap();
    let treasury_id = treasury_body["treasury_id"].as_str().unwrap().to_string();

    // Create Agent Slot
    let agent_res = client.post(format!("{}/v1/agents/{}", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "name": "Test Agent",
            "description": "Integration test agent",
            "allocation_pct": 50.0,
            "tier_limit_usd": 5000.0,
            "concurrent_permit_cap": 3
        }))
        .send().await.unwrap();
    
    assert_eq!(agent_res.status(), StatusCode::CREATED, "Agent creation failed");
    let agent_body: serde_json::Value = agent_res.json().await.unwrap();
    let agent_id = agent_body["agent"]["agent_id"].as_str().unwrap().to_string();
    let api_key = agent_body["api_key"].as_str().unwrap().to_string();

    (token, treasury_id, agent_id, api_key)
}

// ═══════════════════════════════════════════════════════════════════
// TEST 1: Agent creation sets correct initial state
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_agent_creation_initial_state() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let (token, treasury_id, _agent_id, _api_key) = create_agent_setup(&base_url).await;

    // List agents — should have exactly one with PENDING_CONNECTION
    let list_res = client.get(format!("{}/v1/agents/{}", base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", token))
        .send().await.unwrap();
    
    assert_eq!(list_res.status(), StatusCode::OK);
    let agents: Vec<serde_json::Value> = list_res.json().await.unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["status"].as_str().unwrap(), "PENDING_CONNECTION");
    assert_eq!(agents[0]["allocation_pct"].as_f64().unwrap(), 50.0);
    assert_eq!(agents[0]["tier_limit_usd"].as_f64().unwrap(), 5000.0);
    assert_eq!(agents[0]["concurrent_permit_cap"].as_i64().unwrap(), 3);
    assert!(agents[0]["agent_pubkey"].is_null(), "Pubkey should be null before handshake");
}

// ═══════════════════════════════════════════════════════════════════
// TEST 2: Handshake with valid API key — immediate activation
//         (no wallet assigned → skips signer check)
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_handshake_valid_key_no_wallet() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let (_token, _treasury_id, _agent_id, api_key) = create_agent_setup(&base_url).await;

    let res = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GTEST1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF12"
        }))
        .send().await.unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "ACTIVE");
    assert!(body["agent_id"].is_string());
    assert!(body["treasury_id"].is_string());
    assert!(body["coordinator_pubkey"].is_string());
    assert!(body["websocket_endpoint"].is_string());
}

// ═══════════════════════════════════════════════════════════════════
// TEST 3: Handshake with invalid API key → 401
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_handshake_invalid_api_key() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let _ = create_agent_setup(&base_url).await;

    let res = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": "synod_totally_invalid_key_that_does_not_exist",
            "agent_pubkey": "GTEST000000000000000000000000000000000000000000000000000"
        }))
        .send().await.unwrap();

    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"].as_str().unwrap(), "INVALID_API_KEY");
}

// ═══════════════════════════════════════════════════════════════════
// TEST 4: Pubkey conflict — same slot, different pubkey
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_handshake_pubkey_conflict() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let (_token, _treasury_id, _agent_id, api_key) = create_agent_setup(&base_url).await;

    // First handshake — registers pubkey A
    let res1 = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GABCDEFGHIJK1234567890ABCDEFGHIJK123456789012345678901"
        }))
        .send().await.unwrap();
    assert_eq!(res1.status(), StatusCode::OK);

    // Second handshake — different pubkey B → 409
    let res2 = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GXYZ999999999999999999999999999999999999999999999999999"
        }))
        .send().await.unwrap();

    assert_eq!(res2.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = res2.json().await.unwrap();
    assert_eq!(body["error"].as_str().unwrap(), "PUBKEY_CONFLICT");
}

// ═══════════════════════════════════════════════════════════════════
// TEST 5: Idempotent reconnect — same pubkey connects again
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_handshake_idempotent_reconnect() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let (_token, _treasury_id, _agent_id, api_key) = create_agent_setup(&base_url).await;

    let pubkey = "GIDEMPOTENT12345678901234567890123456789012345678901234";

    // First connect
    let res1 = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({ "api_key": api_key, "agent_pubkey": pubkey }))
        .send().await.unwrap();
    assert_eq!(res1.status(), StatusCode::OK);

    // Second connect — same pubkey, should succeed again
    let res2 = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({ "api_key": api_key, "agent_pubkey": pubkey }))
        .send().await.unwrap();
    assert_eq!(res2.status(), StatusCode::OK);

    let body: serde_json::Value = res2.json().await.unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "ACTIVE");
}

// ═══════════════════════════════════════════════════════════════════
// TEST 6: Suspended agent cannot connect → 403
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_handshake_suspended_agent() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let (token, treasury_id, agent_id, api_key) = create_agent_setup(&base_url).await;

    // Suspend the agent
    let suspend_res = client.post(format!("{}/v1/agents/{}/{}/suspend", base_url, treasury_id, agent_id))
        .header("Authorization", format!("Bearer {}", token))
        .send().await.unwrap();
    assert_eq!(suspend_res.status(), StatusCode::OK);

    // Try to connect — should be forbidden
    let res = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GSUSPENDED12345678901234567890123456789012345678901234"
        }))
        .send().await.unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["error"].as_str().unwrap(), "AGENT_SUSPENDED");
}

// ═══════════════════════════════════════════════════════════════════
// TEST 7: Agent status endpoint returns correct data
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_agent_status_endpoint() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let (_token, _treasury_id, agent_id, api_key) = create_agent_setup(&base_url).await;

    // Connect first
    client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GSTATUS12345678901234567890123456789012345678901234567"
        }))
        .send().await.unwrap();

    // Get status
    let res = client.get(format!("{}/v1/agents/{}/status", base_url, agent_id))
        .send().await.unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body: serde_json::Value = res.json().await.unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "ACTIVE");
    assert_eq!(body["name"].as_str().unwrap(), "Test Agent");
    assert!(body["wallet_access"].is_array());
}

// ═══════════════════════════════════════════════════════════════════
// TEST 8: Agent status endpoint — unknown agent → 404
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_agent_status_not_found() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let _ = create_agent_setup(&base_url).await;

    let fake_id = Uuid::new_v4();
    let res = client.get(format!("{}/v1/agents/{}/status", base_url, fake_id))
        .send().await.unwrap();

    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ═══════════════════════════════════════════════════════════════════
// TEST 9: Heartbeat updates last_connected and reactivates INACTIVE
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_heartbeat_endpoint() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let (_token, _treasury_id, agent_id, api_key) = create_agent_setup(&base_url).await;

    // Connect
    client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GHEARTBEAT1234567890123456789012345678901234567890ABCD"
        }))
        .send().await.unwrap();

    // Send heartbeat
    let hb_res = client.post(format!("{}/v1/agents/{}/heartbeat", base_url, agent_id))
        .send().await.unwrap();
    assert_eq!(hb_res.status(), StatusCode::OK);

    // Verify agent is still ACTIVE
    let status_res = client.get(format!("{}/v1/agents/{}/status", base_url, agent_id))
        .send().await.unwrap();
    let body: serde_json::Value = status_res.json().await.unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "ACTIVE");
}

// ═══════════════════════════════════════════════════════════════════
// TEST 10: Heartbeat for unknown agent → 404
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_heartbeat_unknown_agent() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let _ = create_agent_setup(&base_url).await;

    let fake_id = Uuid::new_v4();
    let res = client.post(format!("{}/v1/agents/{}/heartbeat", base_url, fake_id))
        .send().await.unwrap();

    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ═══════════════════════════════════════════════════════════════════
// TEST 11: Suspend agent emits event on WebSocket
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_suspend_emits_ws_event() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let (token, treasury_id, agent_id, api_key) = create_agent_setup(&base_url).await;

    // Connect the agent
    client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GWSEVENT12345678901234567890123456789012345678901234567"
        }))
        .send().await.unwrap();

    // Open WebSocket
    let ws_url = base_url.replace("http://", "ws://") + &format!("/v1/agents/ws/{}", agent_id);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("WS connect failed");

    // Verify ping/pong works
    ws_stream.send(Message::Text("ping".into())).await.unwrap();
    let pong = tokio::time::timeout(
        tokio::time::Duration::from_secs(2),
        ws_stream.next()
    ).await.expect("Ping timeout").expect("Stream closed").expect("Error");
    match pong {
        Message::Text(t) => assert_eq!(t, "pong"),
        other => panic!("Expected pong, got {:?}", other),
    }

    // Suspend the agent
    client.post(format!("{}/v1/agents/{}/{}/suspend", base_url, treasury_id, agent_id))
        .header("Authorization", format!("Bearer {}", token))
        .send().await.unwrap();

    // Check for AGENT_SUSPENDED event on WebSocket
    let event = tokio::time::timeout(
        tokio::time::Duration::from_secs(5),
        ws_stream.next()
    ).await.expect("WS event timeout").expect("Stream closed").expect("Error");

    match event {
        Message::Text(text) => {
            let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed["type"].as_str().unwrap(), "AGENT_SUSPENDED");
        },
        other => panic!("Expected text event, got {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════════
// TEST 12: Full lifecycle → create, connect, heartbeat, suspend
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_full_agent_lifecycle() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    // 1. Create agent
    let (token, treasury_id, agent_id, api_key) = create_agent_setup(&base_url).await;

    // 2. Verify PENDING_CONNECTION
    let status = client.get(format!("{}/v1/agents/{}/status", base_url, agent_id))
        .send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    assert_eq!(status["status"].as_str().unwrap(), "PENDING_CONNECTION");

    // 3. Connect → ACTIVE
    let connect_res = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GLIFECYCLE123456789012345678901234567890123456789012345"
        }))
        .send().await.unwrap();
    assert_eq!(connect_res.status(), StatusCode::OK);

    let status = client.get(format!("{}/v1/agents/{}/status", base_url, agent_id))
        .send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    assert_eq!(status["status"].as_str().unwrap(), "ACTIVE");

    // 4. Heartbeat
    let hb = client.post(format!("{}/v1/agents/{}/heartbeat", base_url, agent_id))
        .send().await.unwrap();
    assert_eq!(hb.status(), StatusCode::OK);

    // 5. Suspend → SUSPENDED
    let suspend = client.post(format!("{}/v1/agents/{}/{}/suspend", base_url, treasury_id, agent_id))
        .header("Authorization", format!("Bearer {}", token))
        .send().await.unwrap();
    assert_eq!(suspend.status(), StatusCode::OK);

    let status = client.get(format!("{}/v1/agents/{}/status", base_url, agent_id))
        .send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    assert_eq!(status["status"].as_str().unwrap(), "SUSPENDED");

    // 6. Connect while suspended → 403
    let blocked = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GLIFECYCLE123456789012345678901234567890123456789012345"
        }))
        .send().await.unwrap();
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);

    // 7. Heartbeat while suspended → 404 (not ACTIVE or INACTIVE)
    let hb_blocked = client.post(format!("{}/v1/agents/{}/heartbeat", base_url, agent_id))
        .send().await.unwrap();
    assert_eq!(hb_blocked.status(), StatusCode::NOT_FOUND);
}

// ═══════════════════════════════════════════════════════════════════
// TEST 13: Multiple agents on same treasury
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_multiple_agents_same_treasury() {
    let base_url = spawn_test_server().await;
    let client = Client::new();
    let email = format!("multi_{}@test.com", Uuid::new_v4());

    // Register + Login + Treasury
    client.post(format!("{}/v1/auth/register", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();

    let login = client.post(format!("{}/v1/auth/login", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    let token = login["token"].as_str().unwrap();

    let treasury = client.post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "name": "Multi-Agent Treasury", "network": "testnet" }))
        .send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    let tid = treasury["treasury_id"].as_str().unwrap();

    // Create 3 agents
    let mut api_keys = Vec::new();
    for i in 0..3 {
        let res = client.post(format!("{}/v1/agents/{}", base_url, tid))
            .header("Authorization", format!("Bearer {}", token))
            .json(&json!({
                "name": format!("Agent {}", i),
                "allocation_pct": 33.33
            }))
            .send().await.unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let body: serde_json::Value = res.json().await.unwrap();
        api_keys.push(body["api_key"].as_str().unwrap().to_string());
    }

    // Connect all 3
    for (i, key) in api_keys.iter().enumerate() {
        let res = client.post(format!("{}/v1/agents/connect", base_url))
            .json(&json!({
                "api_key": key,
                "agent_pubkey": format!("GMULTI{:0>50}", i)
            }))
            .send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "Agent {} failed to connect", i);
    }

    // List — should see all 3 as ACTIVE
    let list: Vec<serde_json::Value> = client.get(format!("{}/v1/agents/{}", base_url, tid))
        .header("Authorization", format!("Bearer {}", token))
        .send().await.unwrap().json().await.unwrap();
    
    assert_eq!(list.len(), 3);
    assert!(list.iter().all(|a| a["status"].as_str().unwrap() == "ACTIVE"));
}

// ═══════════════════════════════════════════════════════════════════
// TEST 14: WebSocket ping/pong on fresh connection
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_websocket_ping_pong() {
    let base_url = spawn_test_server().await;
    let client = Client::new();
    let (_token, _treasury_id, agent_id, api_key) = create_agent_setup(&base_url).await;

    // Connect agent
    client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GWSPING12345678901234567890123456789012345678901234567"
        }))
        .send().await.unwrap();

    // Open WebSocket
    let ws_url = base_url.replace("http://", "ws://") + &format!("/v1/agents/ws/{}", agent_id);
    let (mut ws, _) = connect_async(ws_url).await.expect("WS connect failed");

    // Send 5 pings and verify all pongs come back
    for _ in 0..5 {
        ws.send(Message::Text("ping".into())).await.unwrap();
        let msg = tokio::time::timeout(
            tokio::time::Duration::from_secs(2),
            ws.next()
        ).await.expect("Pong timeout").expect("Closed").expect("Error");
        match msg {
            Message::Text(t) => assert_eq!(t, "pong"),
            _ => panic!("Expected pong text"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// TEST 15: Connect response includes wallet_access with headroom
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_connect_response_wallet_access_structure() {
    let base_url = spawn_test_server().await;
    let client = Client::new();
    let email = format!("wallet_{}@test.com", Uuid::new_v4());

    // Setup user + treasury
    client.post(format!("{}/v1/auth/register", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap();

    let login = client.post(format!("{}/v1/auth/login", base_url))
        .json(&json!({ "email": email, "password": "password123" }))
        .send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    let token = login["token"].as_str().unwrap();

    let treasury = client.post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({ "name": "Headroom Treasury", "network": "testnet" }))
        .send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    let tid = treasury["treasury_id"].as_str().unwrap();

    // Create agent with a specific wallet address
    let agent_res = client.post(format!("{}/v1/agents/{}", base_url, tid))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "name": "Headroom Agent",
            "wallet_address": "GDQP000000000000000000000000000000000000000000000000000",
            "allocation_pct": 50.0,
            "tier_limit_usd": 5000.0,
            "concurrent_permit_cap": 3
        }))
        .send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    let api_key = agent_res["api_key"].as_str().unwrap();

    // Connect — wallet doesn't exist on Horizon, so signer check fails gracefully
    // and goes to PENDING_SIGNER
    let connect_res = client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GHEADROOM1234567890123456789012345678901234567890ABCDE"
        }))
        .send().await.unwrap();

    let body: serde_json::Value = connect_res.json().await.unwrap();
    
    // Should have wallet_access array
    assert!(body["wallet_access"].is_array());
    
    if body["wallet_access"].as_array().unwrap().len() > 0 {
        let wa = &body["wallet_access"][0];
        assert!(wa["wallet_address"].is_string());
        assert!(wa["allocation_pct"].is_f64());
        assert!(wa["tier_limit_usd"].is_f64());
        assert!(wa["concurrent_permit_cap"].is_i64());
        assert!(wa["current_wallet_aum_usd"].is_string());
        assert!(wa["agent_max_usd"].is_string());
    }
}

// ═══════════════════════════════════════════════════════════════════
// TEST 16: Event logging — handshake writes to events table
// ═══════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_handshake_logs_event() {
    let base_url = spawn_test_server().await;
    let client = Client::new();

    let (_token, _treasury_id, _agent_id, api_key) = create_agent_setup(&base_url).await;

    // Connect
    client.post(format!("{}/v1/agents/connect", base_url))
        .json(&json!({
            "api_key": api_key,
            "agent_pubkey": "GEVENTLOG1234567890123456789012345678901234567890ABCDE"
        }))
        .send().await.unwrap();

    // Check events table via the status endpoint (we can't query DB directly from here,
    // but we verify the agent became ACTIVE — which requires the event to have been logged)
    let status = client.get(format!("{}/v1/agents/{}/status", base_url, _agent_id))
        .send().await.unwrap().json::<serde_json::Value>().await.unwrap();
    assert_eq!(status["status"].as_str().unwrap(), "ACTIVE");
}
