use axum::http::StatusCode;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

mod common;
use common::{
    attach_active_wallet, connect_agent, connect_agent_mcp, create_agent_slot, create_treasury,
    enroll_agent_pubkey, generate_test_stellar_keypair, setup_test_context,
};

#[serial_test::serial]
#[tokio::test]
async fn test_agent_slot_starts_pending_pubkey() {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "Agent Treasury").await;
    let (_agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();
    let agent_id = create_agent_slot(&ctx, treasury_id, "Test Agent", &agent_pubkey).await;

    let response = ctx.client
        .get(format!("{}/v1/agents/{}", ctx.base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let agents: Vec<serde_json::Value> = response.json().await.unwrap();
    let agent = agents.iter().find(|item| item["agent_id"].as_str() == Some(&agent_id.to_string())).unwrap();
    assert_eq!(agent["status"].as_str().unwrap(), "PENDING_CONFIGURATION");
    assert_eq!(agent["agent_pubkey"].as_str().unwrap(), agent_pubkey);
}

#[serial_test::serial]
#[tokio::test]
async fn test_pubkey_enrollment_and_connect_issue_session() {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "Connect Treasury").await;
    let (wallet_signing_key, wallet_address) = generate_test_stellar_keypair();
    let (agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();

    attach_active_wallet(&ctx, treasury_id, &wallet_address).await;
    let agent_id = create_agent_slot(&ctx, treasury_id, "Connect Agent", &agent_pubkey).await;

    let enrolled = enroll_agent_pubkey(&ctx, agent_id, &wallet_address, &wallet_signing_key, &agent_pubkey).await;
    assert_eq!(enrolled["agent_pubkey"].as_str().unwrap(), agent_pubkey);

    let connected = connect_agent(&ctx, &agent_pubkey, &agent_signing_key).await;
    assert_eq!(connected["agent_id"].as_str().unwrap(), agent_id.to_string());
    assert_eq!(connected["slot_status"].as_str().unwrap(), "PENDING_CONFIGURATION");
    assert_eq!(connected["connection_phase"].as_str().unwrap(), "PENDING");
    assert!(connected["session_token"].as_str().is_some());
    assert!(connected["websocket_token"].as_str().is_some());
}

#[serial_test::serial]
#[tokio::test]
async fn test_connect_challenge_is_single_use() {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "Replay Treasury").await;
    let (wallet_signing_key, wallet_address) = generate_test_stellar_keypair();
    let (agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();

    attach_active_wallet(&ctx, treasury_id, &wallet_address).await;
    let agent_id = create_agent_slot(&ctx, treasury_id, "Replay Agent", &agent_pubkey).await;
    enroll_agent_pubkey(&ctx, agent_id, &wallet_address, &wallet_signing_key, &agent_pubkey).await;

    let challenge_response = ctx.client
        .post(format!("{}/v1/agents/connect/challenge", ctx.base_url))
        .json(&json!({ "agent_pubkey": agent_pubkey }))
        .send()
        .await
        .unwrap();
    let challenge_body: serde_json::Value = challenge_response.json().await.unwrap();
    let challenge = challenge_body["challenge"].as_str().unwrap();
    let message = format!(
        "synod-connect:{}:{}:{}:{}",
        challenge_body["agent_id"].as_str().unwrap(),
        challenge_body["treasury_id"].as_str().unwrap(),
        agent_pubkey,
        challenge
    );
    let signature = common::sign_with_key(&agent_signing_key, message.as_bytes());

    let first = ctx.client
        .post(format!("{}/v1/agents/connect/complete", ctx.base_url))
        .json(&json!({
            "agent_pubkey": agent_pubkey,
            "challenge": challenge,
            "signature": signature,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let second = ctx.client
        .post(format!("{}/v1/agents/connect/complete", ctx.base_url))
        .json(&json!({
            "agent_pubkey": agent_pubkey,
            "challenge": challenge,
            "signature": signature,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
    let body: serde_json::Value = second.json().await.unwrap();
    assert_eq!(body["error"].as_str().unwrap(), "CHALLENGE_EXPIRED");
}

#[serial_test::serial]
#[tokio::test]
async fn test_websocket_requires_valid_ticket_and_accepts_ping() {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "WS Treasury").await;
    let (wallet_signing_key, wallet_address) = generate_test_stellar_keypair();
    let (agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();

    attach_active_wallet(&ctx, treasury_id, &wallet_address).await;
    let agent_id = create_agent_slot(&ctx, treasury_id, "WS Agent", &agent_pubkey).await;
    enroll_agent_pubkey(&ctx, agent_id, &wallet_address, &wallet_signing_key, &agent_pubkey).await;
    let connected = connect_agent(&ctx, &agent_pubkey, &agent_signing_key).await;

    let invalid_url = ctx.base_url.replace("http://", "ws://") + &format!("/v1/agents/ws/{}?token=bad-token", agent_id);
    assert!(connect_async(invalid_url).await.is_err());

    let ws_token = connected["websocket_token"].as_str().unwrap();
    let ws_url = ctx.base_url.replace("http://", "ws://") + &format!("/v1/agents/ws/{}?token={}", agent_id, ws_token);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("ws should connect");

    ws_stream.send(Message::Text("ping".into())).await.unwrap();
    let message = tokio::time::timeout(tokio::time::Duration::from_secs(2), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    match message {
        Message::Text(text) => assert_eq!(text, "pong"),
        other => panic!("expected pong, got {:?}", other),
    }
}

#[serial_test::serial]
#[tokio::test]
async fn test_suspended_agent_cannot_start_connect_challenge() {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "Suspended Treasury").await;
    let (wallet_signing_key, wallet_address) = generate_test_stellar_keypair();
    let (_agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();

    attach_active_wallet(&ctx, treasury_id, &wallet_address).await;
    let agent_id = create_agent_slot(&ctx, treasury_id, "Suspended Agent", &agent_pubkey).await;
    enroll_agent_pubkey(&ctx, agent_id, &wallet_address, &wallet_signing_key, &agent_pubkey).await;

    let suspend_response = ctx.client
        .post(format!("{}/v1/agents/{}/{}/suspend", ctx.base_url, treasury_id, agent_id))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .send()
        .await
        .unwrap();
    assert_eq!(suspend_response.status(), StatusCode::OK);

    let challenge_response = ctx.client
        .post(format!("{}/v1/agents/connect/challenge", ctx.base_url))
        .json(&json!({ "agent_pubkey": agent_pubkey }))
        .send()
        .await
        .unwrap();
    assert_eq!(challenge_response.status(), StatusCode::FORBIDDEN);
}

#[serial_test::serial]
#[tokio::test]
async fn test_mcp_ready_flips_immediately_after_slot_creation() {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "MCP Ready Treasury").await;
    let (_agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();

    create_agent_slot(&ctx, treasury_id, "MCP Ready Agent", &agent_pubkey).await;

    let response = ctx.client
        .get(format!("{}/connect/status", ctx.base_url))
        .query(&[("public_key", agent_pubkey.as_str())])
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "ready");
    assert_eq!(body["connect_allowed"].as_bool().unwrap(), true);
}

#[serial_test::serial]
#[tokio::test]
async fn test_mcp_connect_handshake_and_ws_work_with_slot_only() {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "MCP Connect Treasury").await;
    let (agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();
    let agent_id = create_agent_slot(&ctx, treasury_id, "MCP Connect Agent", &agent_pubkey).await;

    let connected = connect_agent_mcp(&ctx, &agent_pubkey, &agent_signing_key).await;
    assert_eq!(connected["agent_id"].as_str().unwrap(), agent_id.to_string());
    assert!(connected["ws_ticket"].as_str().is_some());

    let ws_ticket = connected["ws_ticket"].as_str().unwrap();
    let ws_url = ctx.base_url.replace("http://", "ws://") + &format!("/agent/ws?ticket={}", ws_ticket);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("mcp ws should connect");

    ws_stream.send(Message::Text("ping".into())).await.unwrap();
    let message = tokio::time::timeout(tokio::time::Duration::from_secs(2), ws_stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    match message {
        Message::Text(text) => assert_eq!(text, "pong"),
        other => panic!("expected pong, got {:?}", other),
    }
}
