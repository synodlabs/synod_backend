use bigdecimal::BigDecimal;
use reqwest::StatusCode;
use serde::Serialize;
use synod_shared::models::PermitRequest;
use uuid::Uuid;

mod common;
use common::{
    attach_active_wallet, build_signed_request_auth, connect_agent, create_agent_slot,
    create_treasury, enroll_agent_pubkey, generate_test_stellar_keypair, setup_test_context,
};

#[derive(Serialize)]
struct OutcomeSignaturePayload {
    tx_hash: String,
    pnl_usd: String,
    final_amount_units: String,
}

async fn setup_halt_agent() -> (
    common::TestContext,
    Uuid,
    Uuid,
    String,
    ed25519_dalek::SigningKey,
    String,
    String,
) {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "Halt Treasury").await;
    let (wallet_signing_key, wallet_address) = generate_test_stellar_keypair();
    let (agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();

    attach_active_wallet(&ctx, treasury_id, &wallet_address).await;
    sqlx::query("UPDATE treasuries SET current_aum_usd = 10000, peak_aum_usd = 10000 WHERE treasury_id = $1")
        .bind(treasury_id)
        .execute(&ctx.db)
        .await
        .unwrap();

    let agent_id = create_agent_slot(&ctx, treasury_id, "Halt Agent", &agent_pubkey).await;
    enroll_agent_pubkey(
        &ctx,
        agent_id,
        &wallet_address,
        &wallet_signing_key,
        &agent_pubkey,
    )
    .await;
    let connect_data = connect_agent(&ctx, &agent_pubkey, &agent_signing_key).await;
    let session_token = connect_data["session_token"].as_str().unwrap().to_string();

    let constitution_response = ctx
        .client
        .post(format!(
            "{}/v1/treasuries/{}/constitution",
            ctx.base_url, treasury_id
        ))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .json(&serde_json::json!({
            "content": {
                "memo": "Halt constitution",
                "treasury_rules": {
                    "max_drawdown_pct": 15.0,
                    "max_concurrent_permits": 10
                },
                "agent_wallet_rules": [{
                    "agent_id": agent_id,
                    "wallet_address": wallet_address,
                    "allocation_pct": 50.0,
                    "tier_limit_usd": 5000.0,
                    "concurrent_permit_cap": 3
                }]
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(constitution_response.status(), StatusCode::CREATED);

    sqlx::query("UPDATE agent_slots SET status = 'ACTIVE' WHERE agent_id = $1")
        .bind(agent_id)
        .execute(&ctx.db)
        .await
        .unwrap();

    (
        ctx,
        treasury_id,
        agent_id,
        wallet_address,
        agent_signing_key,
        agent_pubkey,
        session_token,
    )
}

#[serial_test::serial]
#[tokio::test]
async fn test_phase_9_halt_and_resume() {
    let (
        ctx,
        treasury_id,
        agent_id,
        wallet_address,
        agent_signing_key,
        agent_pubkey,
        session_token,
    ) = setup_halt_agent().await;

    let permit = PermitRequest {
        agent_id,
        treasury_id,
        wallet_address: wallet_address.clone(),
        asset_code: "XLM".into(),
        asset_issuer: None,
        requested_amount: BigDecimal::from(500),
    };

    let permit_response = ctx.client
        .post(format!("{}/v1/permits/request", ctx.base_url))
        .header("Authorization", format!("Bearer {}", session_token))
        .json(&serde_json::json!({
            "agent_id": permit.agent_id,
            "treasury_id": permit.treasury_id,
            "wallet_address": permit.wallet_address,
            "asset_code": permit.asset_code,
            "asset_issuer": permit.asset_issuer,
            "requested_amount": permit.requested_amount,
            "request_auth": build_signed_request_auth(&agent_signing_key, &agent_pubkey, agent_id, "permit.request", &permit),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(permit_response.status(), StatusCode::CREATED);
    let permit_body: serde_json::Value = permit_response.json().await.unwrap();
    let permit_id = Uuid::parse_str(permit_body["permit_id"].as_str().unwrap()).unwrap();

    let outcome_response = ctx.client
        .post(format!("{}/v1/permits/{}/outcome", ctx.base_url, permit_id))
        .header("Authorization", format!("Bearer {}", session_token))
        .json(&{
            let signed_outcome_payload = OutcomeSignaturePayload {
                tx_hash: "LOSS_HASH".to_string(),
                pnl_usd: "-2000".to_string(),
                final_amount_units: "0".to_string(),
            };
            serde_json::json!({
                "tx_hash": "LOSS_HASH",
                "pnl_usd": BigDecimal::from(-2000),
                "final_amount_units": BigDecimal::from(0),
                "request_auth": build_signed_request_auth(&agent_signing_key, &agent_pubkey, agent_id, "permit.outcome", &signed_outcome_payload),
            })
        })
        .send()
        .await
        .unwrap();
    assert_eq!(outcome_response.status(), StatusCode::OK);

    let health: String = sqlx::query_scalar("SELECT health FROM treasuries WHERE treasury_id = $1")
        .bind(treasury_id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
    assert_eq!(health, "HALTED");

    let blocked_permit = PermitRequest {
        agent_id,
        treasury_id,
        wallet_address: wallet_address.clone(),
        asset_code: "XLM".into(),
        asset_issuer: None,
        requested_amount: BigDecimal::from(100),
    };
    let blocked = ctx.client
        .post(format!("{}/v1/permits/request", ctx.base_url))
        .header("Authorization", format!("Bearer {}", session_token))
        .json(&serde_json::json!({
            "agent_id": blocked_permit.agent_id,
            "treasury_id": blocked_permit.treasury_id,
            "wallet_address": blocked_permit.wallet_address,
            "asset_code": blocked_permit.asset_code,
            "asset_issuer": blocked_permit.asset_issuer,
            "requested_amount": blocked_permit.requested_amount,
            "request_auth": build_signed_request_auth(&agent_signing_key, &agent_pubkey, agent_id, "permit.request", &blocked_permit),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::OK);
    let blocked_body: serde_json::Value = blocked.json().await.unwrap();
    assert_eq!(blocked_body["approved"].as_bool().unwrap(), false);
    assert_eq!(
        blocked_body["deny_reason"].as_str().unwrap(),
        "TREASURY_HALTED"
    );

    let resume_response = ctx
        .client
        .post(format!(
            "{}/v1/treasuries/{}/resume",
            ctx.base_url, treasury_id
        ))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .send()
        .await
        .unwrap();
    assert_eq!(resume_response.status(), StatusCode::OK);

    let resumed_permit = PermitRequest {
        agent_id,
        treasury_id,
        wallet_address,
        asset_code: "XLM".into(),
        asset_issuer: None,
        requested_amount: BigDecimal::from(100),
    };
    let resumed = ctx.client
        .post(format!("{}/v1/permits/request", ctx.base_url))
        .header("Authorization", format!("Bearer {}", session_token))
        .json(&serde_json::json!({
            "agent_id": resumed_permit.agent_id,
            "treasury_id": resumed_permit.treasury_id,
            "wallet_address": resumed_permit.wallet_address,
            "asset_code": resumed_permit.asset_code,
            "asset_issuer": resumed_permit.asset_issuer,
            "requested_amount": resumed_permit.requested_amount,
            "request_auth": build_signed_request_auth(&agent_signing_key, &agent_pubkey, agent_id, "permit.request", &resumed_permit),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::CREATED);
}
