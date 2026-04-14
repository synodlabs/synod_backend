use reqwest::StatusCode;
use uuid::Uuid;
use crate::common::setup_test_context;
use tokio_tungstenite::connect_async;
use http::Request;

mod common;

#[serial_test::serial]
#[tokio::test]
async fn test_phase_10_dashboard_and_ws() {
    let ctx = setup_test_context().await;
    let base_url = ctx.base_url.clone();
    let client = &ctx.client;
    let auth_header = format!("Bearer {}", ctx.user_token);

    // 1. Create Treasury
    let treasury_resp = client.post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", &auth_header)
        .json(&serde_json::json!({
            "name": "Dashboard Test Treasury",
            "network": "testnet"
        }))
        .send().await.unwrap();
    let treasury_data: serde_json::Value = treasury_resp.json().await.unwrap();
    let treasury_id = treasury_data["treasury_id"].as_str().unwrap();
    let treasury_uuid = Uuid::parse_str(treasury_id).unwrap();

    // 2. Mock some state (AUM and Constitution)
    sqlx::query("UPDATE treasuries SET current_aum_usd = 50000, peak_aum_usd = 50000, constitution_version = 1 WHERE treasury_id = $1")
        .bind(treasury_uuid)
        .execute(&ctx.db).await.unwrap();

    sqlx::query(
        "INSERT INTO constitution_history (treasury_id, version, content, state_hash, executed_at) VALUES ($1, 1, $2, 'hash', NOW())"
    )
    .bind(treasury_uuid)
    .bind(serde_json::json!({
        "agent_allocations": []
    }))
    .execute(&ctx.db).await.unwrap();

    // 3. Test REST API: List Treasuries
    let list_resp = client.get(format!("{}/v1/dashboard", base_url))
        .header("Authorization", &auth_header)
        .send().await.unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);
    let list_data: Vec<serde_json::Value> = list_resp.json().await.unwrap();
    assert!(list_data.iter().any(|t| t["treasury_id"] == treasury_id));

    // 4. Test REST API: Get Treasury State
    let state_resp = client.get(format!("{}/v1/dashboard/{}", base_url, treasury_id))
        .header("Authorization", &auth_header)
        .send().await.unwrap();
    assert_eq!(state_resp.status(), StatusCode::OK);
    let state_data: serde_json::Value = state_resp.json().await.unwrap();
    assert_eq!(state_data["name"], "Dashboard Test Treasury");
    assert_eq!(state_data["current_aum_usd"], 50000.0);

    // 5. Test WebSocket Connection
    let ws_url = base_url.replace("http://", "ws://") + "/v1/dashboard/ws";
    let host = base_url.replace("http://", "");
    
    let request = Request::builder()
        .uri(&ws_url)
        .header("Host", host)
        .header("Authorization", &auth_header)
        .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .body(())
        .unwrap();

    let (_ws_stream, _) = connect_async(request).await.expect("Failed to connect to WS");

    // 6. WS handshake success is enough for this integration test.
}
