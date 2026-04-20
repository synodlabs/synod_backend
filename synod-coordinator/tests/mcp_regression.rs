use axum::http::StatusCode;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

mod common;
use common::{
    attach_active_wallet, build_signed_test_payment_envelope_xdr, connect_agent_mcp,
    create_agent_slot, create_treasury, generate_test_stellar_keypair, setup_test_context,
    sign_raw_bytes, spawn_mock_horizon_server,
};

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).unwrap()
        }
        Value::Array(values) => {
            let items = values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",");
            format!("[{}]", items)
        }
        Value::Object(map) => {
            let mut entries = map.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let items = entries
                .into_iter()
                .map(|(key, nested)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap(),
                        canonical_json(nested)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{}}}", items)
        }
    }
}

#[serial_test::serial]
#[tokio::test]
async fn test_mcp_connect_marks_policy_assigned_agent_active_and_allows_intents() {
    let horizon_url = spawn_mock_horizon_server("mcp-flow-hash").await;
    std::env::set_var("SYNOD_TEST_HORIZON_URL", &horizon_url);
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "MCP Regression Treasury").await;
    let (_wallet_signing_key, wallet_address) = generate_test_stellar_keypair();
    let (agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();
    let (_destination_signing_key, destination_address) = generate_test_stellar_keypair();

    attach_active_wallet(&ctx, treasury_id, &wallet_address).await;
    let agent_id =
        create_agent_slot(&ctx, treasury_id, "MCP Regression Agent", &agent_pubkey).await;
    sqlx::query("UPDATE treasuries SET current_aum_usd = 1000, peak_aum_usd = 1000 WHERE treasury_id = $1")
        .bind(treasury_id)
        .execute(&ctx.db)
        .await
        .unwrap();

    let constitution_response = ctx
        .client
        .put(format!(
            "{}/v1/treasuries/{}/constitution",
            ctx.base_url, treasury_id
        ))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .json(&json!({
            "content": {
                "treasury_rules": {
                    "max_drawdown_pct": 20.0,
                    "max_concurrent_permits": 10
                },
                "agent_wallet_rules": [{
                    "agent_id": agent_id,
                    "wallet_address": wallet_address,
                    "allocation_pct": 100.0,
                    "tier_limit_usd": 1000.0,
                    "concurrent_permit_cap": 3
                }],
                "memo": null
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(constitution_response.status(), StatusCode::CREATED);

    let connected = connect_agent_mcp(&ctx, &agent_pubkey, &agent_signing_key).await;
    assert_eq!(
        connected["agent_id"].as_str().unwrap(),
        agent_id.to_string()
    );

    let stored_status: String =
        sqlx::query_scalar("SELECT status FROM agent_slots WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&ctx.db)
            .await
            .unwrap();
    assert_eq!(stored_status, "ACTIVE");

    let intent = json!({
        "type": "payment",
        "to": destination_address,
        "amount": "10",
        "asset": "XLM",
        "wallet_address": wallet_address,
    });
    let signature = sign_raw_bytes(&agent_signing_key, canonical_json(&intent).as_bytes());
    let signed_transaction_xdr = build_signed_test_payment_envelope_xdr(
        &wallet_address,
        &destination_address,
        10 * 10_000_000,
        &agent_signing_key,
    );

    let submit_response = ctx
        .client
        .post(format!("{}/intents/submit", ctx.base_url))
        .json(&json!({
            "public_key": agent_pubkey,
            "signature": signature,
            "intent": intent,
            "signed_transaction_xdr": signed_transaction_xdr,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(submit_response.status(), StatusCode::OK);

    let body: Value = submit_response.json().await.unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "executed");
    assert_eq!(body["tx_hash"].as_str().unwrap(), "mcp-flow-hash");
    assert!(body["reason"].is_null());

    let mut confirmed_payload = None;
    for _ in 0..10 {
        confirmed_payload = sqlx::query_scalar(
            "SELECT payload FROM events WHERE treasury_id = $1 AND event_type = 'INTENT_CONFIRMED' ORDER BY sequence DESC LIMIT 1",
        )
        .bind(treasury_id)
        .fetch_optional(&ctx.db)
        .await
        .unwrap();
        if confirmed_payload.is_some() {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    let confirmed_payload: Value = confirmed_payload.expect("intent confirmed event should persist");
    assert_eq!(confirmed_payload["tx_hash"], "mcp-flow-hash");

    std::env::remove_var("SYNOD_TEST_HORIZON_URL");
}
