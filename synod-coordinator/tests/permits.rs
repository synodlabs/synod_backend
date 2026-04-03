use bigdecimal::BigDecimal;
use reqwest::StatusCode;
use uuid::Uuid;
use synod_shared::models::*;
use synod_coordinator::permit::{OutcomeReport, PermitGroupRequest};
use crate::common::{setup_test_context, TestContext};

mod common;

#[tokio::test]
async fn test_permit_full_lifecycle() {
    let ctx = setup_test_context().await;
    let client = &ctx.client;
    let base_url = &ctx.base_url;
    let auth_header = format!("Bearer {}", ctx.user_token);

    // 1. Create Treasury
    let treasury_resp = client.post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", &auth_header)
        .json(&serde_json::json!({ "name": "Test Treasury", "network": "testnet" }))
        .send().await.unwrap();
    let treasury_data: serde_json::Value = treasury_resp.json().await.unwrap();
    let treasury_id = treasury_data["treasury_id"].as_str().unwrap();
    let treasury_uuid = Uuid::parse_str(treasury_id).unwrap();

    // 1.5 Set Treasury AUM (Required for policy ceiling/floor checks)
    sqlx::query("UPDATE treasuries SET current_aum_usd = 10000, peak_aum_usd = 10000 WHERE treasury_id = $1")
        .bind(treasury_uuid)
        .execute(&ctx.db).await.unwrap();

    // 2. Setup Constitution (Required for policy engine pool lookup)
    let const_resp = client.post(format!("{}/v1/treasuries/{}/constitution", base_url, treasury_id))
        .header("Authorization", &auth_header)
        .json(&serde_json::json!({
            "content": {
                "agent_allocations": []
            }
        }))
        .send().await.unwrap();
    assert_eq!(const_resp.status(), StatusCode::CREATED);

    // 3. Provision Agent
    let agent_resp = client.post(format!("{}/v1/agents/{}", base_url, treasury_id))
        .header("Authorization", &auth_header)
        .json(&serde_json::json!({
            "name": "Trading Bot 1",
            "description": Some("Alpha Strategy")
        }))
        .send().await.unwrap();
    assert_eq!(agent_resp.status(), StatusCode::CREATED);
    let agent_data: serde_json::Value = agent_resp.json().await.unwrap();
    let agent_id = agent_data["agent"]["agent_id"].as_str().unwrap();

    // 3.5 Make Agent Active (bypass handshake for test simplicity)
    sqlx::query("UPDATE agent_slots SET status = 'ACTIVE', wallet_address = $1 WHERE agent_id = $2")
        .bind("GA5WNX...")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .execute(&ctx.db).await.unwrap();

    // 4. Request Permit
    let req_payload = PermitRequest {
        agent_id: Uuid::parse_str(agent_id).unwrap(),
        treasury_id: Uuid::parse_str(treasury_id).unwrap(),
        wallet_address: "GA5WNX...".to_string(),
        asset_code: "XLM".to_string(),
        asset_issuer: None,
        requested_amount: BigDecimal::from(500),
    };

    let permit_resp = client.post(format!("{}/v1/permits/request", base_url))
        .header("Authorization", &auth_header)
        .json(&req_payload)
        .send().await.unwrap();
    
    assert_eq!(permit_resp.status(), StatusCode::CREATED);
    let policy_result: PolicyResult = permit_resp.json().await.unwrap();
    assert!(policy_result.approved);
    
    // We need the permit_id for co-signing and outcomes. 
    // Since we don't return it in the current API (we return PolicyResult), 
    // let's fetch it from the DB.
    let permit_id: Uuid = sqlx::query_scalar("SELECT permit_id FROM permits WHERE agent_id = $1 LIMIT 1")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .fetch_one(&ctx.db).await.unwrap();

    // 5. Co-signing Verification
    // Valid request
    let cosign_resp = client.post(format!("{}/v1/permits/{}/cosign", base_url, permit_id))
        .header("Authorization", &auth_header)
        .json(&serde_json::json!({ "xdr": "VALID_STALLAR_XDR_DATA" }))
        .send().await.unwrap();
    assert_eq!(cosign_resp.status(), StatusCode::OK);
    let cosign_data: serde_json::Value = cosign_resp.json().await.unwrap();
    assert_eq!(cosign_data["status"], "SIGNED");
    assert!(cosign_data["signature"].as_str().is_some());

    // Invalid request (mock rejection via XDR keyword)
    let bad_cosign = client.post(format!("{}/v1/permits/{}/cosign", base_url, permit_id))
        .header("Authorization", &auth_header)
        .json(&serde_json::json!({ "xdr": "INVALID_TX_AMOUNT" }))
        .send().await.unwrap();
    assert_eq!(bad_cosign.status(), StatusCode::BAD_REQUEST);

    // 6. Outcome Reporting
    let outcome_resp = client.post(format!("{}/v1/permits/{}/outcome", base_url, permit_id))
        .header("Authorization", &auth_header)
        .json(&OutcomeReport {
            tx_hash: "SUCCESS_HASH".into(),
            pnl_usd: BigDecimal::from(50),
            final_amount_units: BigDecimal::from(500),
        })
        .send().await.unwrap();
    assert_eq!(outcome_resp.status(), StatusCode::OK);

    // Verify state transition to CONSUMED
    let status: String = sqlx::query_scalar("SELECT status FROM permits WHERE permit_id = $1")
        .bind(permit_id)
        .fetch_one(&ctx.db).await.unwrap();
    assert_eq!(status, "CONSUMED");

    // 7. Atomic Multi-wallet Group (Success)
    let group_req = PermitGroupRequest {
        agent_id: Uuid::parse_str(agent_id).unwrap(),
        treasury_id: Uuid::parse_str(treasury_id).unwrap(),
        legs: vec![
            PermitRequest {
                agent_id: Uuid::parse_str(agent_id).unwrap(),
                treasury_id: Uuid::parse_str(treasury_id).unwrap(),
                wallet_address: "GA5WNX...".to_string(),
                asset_code: "XLM".to_string(),
                asset_issuer: None,
                requested_amount: BigDecimal::from(100),
            },
            PermitRequest {
                agent_id: Uuid::parse_str(agent_id).unwrap(),
                treasury_id: Uuid::parse_str(treasury_id).unwrap(),
                wallet_address: "GA5WNX...".to_string(),
                asset_code: "XLM".to_string(),
                asset_issuer: None,
                requested_amount: BigDecimal::from(200),
            }
        ],
        require_all: true,
    };

    let group_resp = client.post(format!("{}/v1/permits/group/request", base_url))
        .header("Authorization", &auth_header)
        .json(&group_req)
        .send().await.unwrap();
    assert_eq!(group_resp.status(), StatusCode::CREATED);
    let group_results: Vec<PolicyResult> = group_resp.json().await.unwrap();
    assert_eq!(group_results.len(), 2);
    assert!(group_results[0].approved);
    assert!(group_results[1].approved);

    // 7.5 Clean up (Consume) these permits so they don't count towards the final check
    sqlx::query("UPDATE permits SET status = 'CONSUMED' WHERE agent_id = $1 AND status = 'ACTIVE'")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .execute(&ctx.db).await.unwrap();

    // 8. Atomic Multi-wallet Group (Failure with require_all)
    // Suspend agent to trigger failure
    sqlx::query("UPDATE agent_slots SET status = 'SUSPENDED' WHERE agent_id = $1")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .execute(&ctx.db).await.unwrap();

    let fail_group_resp = client.post(format!("{}/v1/permits/group/request", base_url))
        .header("Authorization", &auth_header)
        .json(&group_req)
        .send().await.unwrap();
    assert_eq!(fail_group_resp.status(), StatusCode::OK); // Returns results with approved: false
    let fail_results: Vec<PolicyResult> = fail_group_resp.json().await.unwrap();
    assert!(!fail_results[0].approved);
    assert_eq!(fail_results[0].deny_reason, Some("GROUP_REQUIRE_ALL_FAILURE".into()));

    // Verify no permits were created for this failed group
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM permits WHERE agent_id = $1 AND status = 'ACTIVE'")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .fetch_one(&ctx.db).await.unwrap();
    assert_eq!(count, 0); // Previous permits were CONSUMED or rolled back
}
