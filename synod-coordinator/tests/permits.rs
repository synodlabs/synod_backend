use bigdecimal::BigDecimal;
use reqwest::StatusCode;
use synod_shared::models::PermitRequest;
use uuid::Uuid;
use serde::Serialize;

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

async fn setup_active_agent() -> (
    common::TestContext,
    Uuid,
    Uuid,
    String,
    ed25519_dalek::SigningKey,
    String,
    String,
) {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "Permit Treasury").await;
    let (wallet_signing_key, wallet_address) = generate_test_stellar_keypair();
    let (agent_signing_key, agent_pubkey) = generate_test_stellar_keypair();

    attach_active_wallet(&ctx, treasury_id, &wallet_address).await;
    sqlx::query("UPDATE treasuries SET current_aum_usd = 10000, peak_aum_usd = 10000 WHERE treasury_id = $1")
        .bind(treasury_id)
        .execute(&ctx.db)
        .await
        .unwrap();

    let agent_id = create_agent_slot(&ctx, treasury_id, "Permit Agent", &agent_pubkey).await;
    enroll_agent_pubkey(&ctx, agent_id, &wallet_address, &wallet_signing_key, &agent_pubkey).await;
    let connect_data = connect_agent(&ctx, &agent_pubkey, &agent_signing_key).await;
    let session_token = connect_data["session_token"].as_str().unwrap().to_string();

    let constitution_response = ctx.client
        .post(format!("{}/v1/treasuries/{}/constitution", ctx.base_url, treasury_id))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .json(&serde_json::json!({
            "content": {
                "memo": "Permit test constitution",
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
async fn test_permit_full_lifecycle_with_signed_requests() {
    let (ctx, treasury_id, agent_id, wallet_address, agent_signing_key, agent_pubkey, session_token) =
        setup_active_agent().await;

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
    assert_eq!(permit_body["approved"].as_bool().unwrap(), true);

    let xdr = "VALID_STELLAR_XDR_DATA";
    let cosign_response = ctx.client
        .post(format!("{}/v1/permits/{}/cosign", ctx.base_url, permit_id))
        .header("Authorization", format!("Bearer {}", session_token))
        .json(&serde_json::json!({
            "xdr": xdr,
            "request_auth": build_signed_request_auth(&agent_signing_key, &agent_pubkey, agent_id, "permit.cosign", &xdr),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(cosign_response.status(), StatusCode::OK);
    let cosign_body: serde_json::Value = cosign_response.json().await.unwrap();
    assert_eq!(cosign_body["status"].as_str().unwrap(), "SIGNED");

    let outcome_payload = serde_json::json!({
        "tx_hash": cosign_body["tx_hash"].as_str().unwrap(),
        "pnl_usd": BigDecimal::from(50),
        "final_amount_units": BigDecimal::from(500),
    });
    let signed_outcome_payload = OutcomeSignaturePayload {
        tx_hash: cosign_body["tx_hash"].as_str().unwrap().to_string(),
        pnl_usd: "50".to_string(),
        final_amount_units: "500".to_string(),
    };
    let outcome_response = ctx.client
        .post(format!("{}/v1/permits/{}/outcome", ctx.base_url, permit_id))
        .header("Authorization", format!("Bearer {}", session_token))
        .json(&serde_json::json!({
            "tx_hash": outcome_payload["tx_hash"],
            "pnl_usd": outcome_payload["pnl_usd"],
            "final_amount_units": outcome_payload["final_amount_units"],
            "request_auth": build_signed_request_auth(&agent_signing_key, &agent_pubkey, agent_id, "permit.outcome", &signed_outcome_payload),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(outcome_response.status(), StatusCode::OK);

    let permit_status: String = sqlx::query_scalar("SELECT status FROM permits WHERE permit_id = $1")
        .bind(permit_id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
    assert_eq!(permit_status, "CONSUMED");
}

#[serial_test::serial]
#[tokio::test]
async fn test_signed_request_replay_is_rejected() {
    let (ctx, treasury_id, agent_id, wallet_address, agent_signing_key, agent_pubkey, session_token) =
        setup_active_agent().await;

    let permit = PermitRequest {
        agent_id,
        treasury_id,
        wallet_address,
        asset_code: "XLM".into(),
        asset_issuer: None,
        requested_amount: BigDecimal::from(100),
    };
    let request_auth = build_signed_request_auth(&agent_signing_key, &agent_pubkey, agent_id, "permit.request", &permit);
    let body = serde_json::json!({
        "agent_id": permit.agent_id,
        "treasury_id": permit.treasury_id,
        "wallet_address": permit.wallet_address,
        "asset_code": permit.asset_code,
        "asset_issuer": permit.asset_issuer,
        "requested_amount": permit.requested_amount,
        "request_auth": request_auth,
    });

    let first = ctx.client
        .post(format!("{}/v1/permits/request", ctx.base_url))
        .header("Authorization", format!("Bearer {}", session_token))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);

    let second = ctx.client
        .post(format!("{}/v1/permits/request", ctx.base_url))
        .header("Authorization", format!("Bearer {}", session_token))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    let second_body: serde_json::Value = second.json().await.unwrap();
    assert_eq!(second_body["error"].as_str().unwrap(), "REQUEST_REPLAY");
}
