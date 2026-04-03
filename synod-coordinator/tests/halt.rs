use reqwest::StatusCode;
use uuid::Uuid;
use synod_shared::models::*;
use synod_coordinator::permit::OutcomeReport;
use crate::common::{setup_test_context, TestContext};
use bigdecimal::BigDecimal;

mod common;

#[tokio::test]
async fn test_phase_9_halt_and_resume() {
    let ctx = setup_test_context().await;
    let base_url = ctx.base_url.clone();
    let client = &ctx.client;
    let auth_header = format!("Bearer {}", ctx.user_token);

    // 2. Create Treasury & Config AUM
    let treasury_resp = client.post(format!("{}/v1/treasuries", base_url))
        .header("Authorization", &auth_header)
        .json(&serde_json::json!({
            "name": "Halt Test Treasury",
            "network": "testnet"
        }))
        .send().await.unwrap();
    let treasury_data: serde_json::Value = treasury_resp.json().await.unwrap();
    let treasury_id = treasury_data["treasury_id"].as_str().unwrap();
    let treasury_uuid = Uuid::parse_str(treasury_id).unwrap();

    // Set AUM to 10,000
    sqlx::query("UPDATE treasuries SET current_aum_usd = 10000, peak_aum_usd = 10000, constitution_version = 1 WHERE treasury_id = $1")
        .bind(treasury_uuid)
        .execute(&ctx.db).await.unwrap();

    // 3. Set Constitution (15% Max Drawdown)
    sqlx::query(
        "INSERT INTO constitution_history (treasury_id, version, content, state_hash, executed_at) VALUES ($1, 1, $2, 'hash', NOW())"
    )
    .bind(treasury_uuid)
    .bind(serde_json::json!({
        "treasury_id": treasury_id,
        "version": 1,
        "max_drawdown_pct": 15.0,
        "agent_allocations": [],
        "inflow_routing": [],
        "governance_mode": "AUTO"
    }))
    .execute(&ctx.db).await.unwrap();

    // 4. Provision & Active Agent
    let agent_resp = client.post(format!("{}/v1/agents/{}", base_url, treasury_id))
        .header("Authorization", &auth_header)
        .json(&serde_json::json!({ "name": "Halt Bot" }))
        .send().await.unwrap();
    let agent_data: serde_json::Value = agent_resp.json().await.unwrap();
    let agent_id = agent_data["agent"]["agent_id"].as_str().unwrap();

    sqlx::query("UPDATE agent_slots SET status = 'ACTIVE', wallet_address = $1 WHERE agent_id = $2")
        .bind("GA5WNX...")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .execute(&ctx.db).await.unwrap();

    // 5. Create Active Permit
    let permit_resp = client.post(format!("{}/v1/permits/request", base_url))
        .header("Authorization", &auth_header)
        .json(&PermitRequest {
            agent_id: Uuid::parse_str(agent_id).unwrap(),
            treasury_id: treasury_uuid,
            wallet_address: "GA5WNX...".to_string(),
            asset_code: "XLM".to_string(),
            asset_issuer: None,
            requested_amount: BigDecimal::from(500),
        })
        .send().await.unwrap();
    assert_eq!(permit_resp.status(), StatusCode::CREATED);
    
    let permit_id: Uuid = sqlx::query_scalar("SELECT permit_id FROM permits WHERE agent_id = $1 LIMIT 1")
        .bind(Uuid::parse_str(agent_id).unwrap())
        .fetch_one(&ctx.db).await.unwrap();

    // 6. Report Lossy Outcome (Breach 15% mark: Loss of 2000 on 10,000 = 20% DD)
    let outcome_resp = client.post(format!("{}/v1/permits/{}/outcome", base_url, permit_id))
        .header("Authorization", &auth_header)
        .json(&OutcomeReport {
            tx_hash: "LOSS_HASH".into(),
            pnl_usd: BigDecimal::from(-2000), // Loss of 2000
            final_amount_units: BigDecimal::from(0),
        })
        .send().await.unwrap();
    assert_eq!(outcome_resp.status(), StatusCode::OK);

    // 7. Verify HALTED State
    let health: String = sqlx::query_scalar("SELECT health FROM treasuries WHERE treasury_id = $1")
        .bind(treasury_uuid)
        .fetch_one(&ctx.db).await.unwrap();
    assert_eq!(health, "HALTED");

    // 8. Try new permit → Should fail
    let fail_permit = client.post(format!("{}/v1/permits/request", base_url))
        .header("Authorization", &auth_header)
        .json(&PermitRequest {
            agent_id: Uuid::parse_str(agent_id).unwrap(),
            treasury_id: treasury_uuid,
            wallet_address: "GA5WNX...".to_string(),
            asset_code: "XLM".to_string(),
            asset_issuer: None,
            requested_amount: BigDecimal::from(100),
        })
        .send().await.unwrap();
    
    // Policy engine returns 200 with approved: false for rejections
    assert_eq!(fail_permit.status(), StatusCode::OK);
    let policy_res: PolicyResult = fail_permit.json().await.unwrap();
    assert!(!policy_res.approved);
    assert_eq!(policy_res.deny_reason, Some("TREASURY_HALTED".into()));

    // 9. Resume
    let resume_resp = client.post(format!("{}/v1/treasuries/{}/resume", base_url, treasury_id))
        .header("Authorization", &auth_header)
        .send().await.unwrap();
    assert_eq!(resume_resp.status(), StatusCode::OK);

    // Verify Peak Reset
    let peak: f64 = sqlx::query_scalar("SELECT peak_aum_usd::float8 FROM treasuries WHERE treasury_id = $1")
        .bind(treasury_uuid)
        .fetch_one(&ctx.db).await.unwrap();
    assert_eq!(peak, 8000.0); // 10000 - 2000

    // 10. Success after resume
    let success_permit = client.post(format!("{}/v1/permits/request", base_url))
        .header("Authorization", &auth_header)
        .json(&PermitRequest {
            agent_id: Uuid::parse_str(agent_id).unwrap(),
            treasury_id: treasury_uuid,
            wallet_address: "GA5WNX...".to_string(),
            asset_code: "XLM".to_string(),
            asset_issuer: None,
            requested_amount: BigDecimal::from(100),
        })
        .send().await.unwrap();
    assert_eq!(success_permit.status(), StatusCode::CREATED);
}
