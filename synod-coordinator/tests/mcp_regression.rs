use axum::http::StatusCode;
use serde_json::{json, Value};

mod common;
use common::{
    attach_active_wallet, connect_agent_mcp, create_agent_slot, create_treasury,
    generate_test_stellar_keypair, setup_test_context, sign_raw_bytes,
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
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "MCP Regression Treasury").await;
    let (_wallet_signing_key, wallet_address) = generate_test_stellar_keypair();
    let (agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();
    let (_destination_signing_key, destination_address) = generate_test_stellar_keypair();

    attach_active_wallet(&ctx, treasury_id, &wallet_address).await;
    let agent_id =
        create_agent_slot(&ctx, treasury_id, "MCP Regression Agent", &agent_pubkey).await;

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

    let submit_response = ctx
        .client
        .post(format!("{}/intents/submit", ctx.base_url))
        .json(&json!({
            "public_key": agent_pubkey,
            "signature": signature,
            "intent": intent,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(submit_response.status(), StatusCode::OK);

    let body: Value = submit_response.json().await.unwrap();
    assert_eq!(body["status"].as_str().unwrap(), "confirmed");
    assert!(body["reason"].is_null());
}
