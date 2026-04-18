use bigdecimal::BigDecimal;
use synod_coordinator::policy::run_policy_engine;
use synod_shared::models::*;
use uuid::Uuid;

fn mock_request(amount: i64) -> PermitRequest {
    PermitRequest {
        agent_id: Uuid::new_v4(),
        treasury_id: Uuid::new_v4(),
        wallet_address: "stellar_wallet".to_string(),
        asset_code: "XLM".to_string(),
        asset_issuer: None,
        requested_amount: BigDecimal::from(amount),
    }
}

fn mock_treasury(health: TreasuryHealth) -> TreasuryState {
    TreasuryState {
        treasury_id: Uuid::new_v4(),
        health,
        peak_aum_usd: BigDecimal::from(10000),
        current_aum_usd: BigDecimal::from(10000),
        state_hash: "hash".to_string(),
    }
}

fn mock_access(tier_limit: i64) -> AgentWalletAccess {
    AgentWalletAccess {
        agent_id: Uuid::new_v4(),
        wallet_address: "stellar_wallet".to_string(),
        allocation_pct: BigDecimal::from(50),
        tier_limit_usd: BigDecimal::from(tier_limit),
        concurrent_permit_cap: 5,
        can_execute: true,
    }
}

fn mock_constitution() -> Constitution {
    Constitution {
        treasury_id: Uuid::new_v4(),
        version: 1,
        memo: None,
        treasury_rules: TreasuryRules {
            max_drawdown_pct: BigDecimal::from(20),
            max_concurrent_permits: 10,
        },
        agent_wallet_rules: vec![],
    }
}

#[test]
fn test_policy_full_approval() {
    let request = mock_request(500);
    let treasury = mock_treasury(TreasuryHealth::Healthy);
    let access = mock_access(1000);
    let constitution = mock_constitution();
    let reservations = BigDecimal::from(0);

    let result = run_policy_engine(
        &request,
        &treasury,
        &access,
        &constitution,
        &reservations,
        0,
        0,
    );

    assert!(result.approved);
    assert_eq!(result.approved_amount, BigDecimal::from(500));
    assert!(result.partial_reason.is_none());
}

#[test]
fn test_policy_deny_early() {
    let request = mock_request(500);
    let treasury = mock_treasury(TreasuryHealth::Halted); // Rule 1 failure
    let access = mock_access(1000);
    let constitution = mock_constitution();

    let result = run_policy_engine(
        &request,
        &treasury,
        &access,
        &constitution,
        &BigDecimal::from(0),
        0,
        0,
    );

    assert!(!result.approved);
    assert_eq!(result.deny_reason.unwrap(), "TREASURY_HALTED");
    assert_eq!(result.policy_check_number.unwrap(), 1);
}

#[test]
fn test_policy_partial_approval_tier_limit() {
    let request = mock_request(2000);
    let access = mock_access(1000); // Tier limit 1000
    let treasury = mock_treasury(TreasuryHealth::Healthy);
    let constitution = mock_constitution();

    let result = run_policy_engine(
        &request,
        &treasury,
        &access,
        &constitution,
        &BigDecimal::from(0),
        0,
        0,
    );

    assert!(result.approved);
    assert_eq!(result.approved_amount, BigDecimal::from(1000));
    assert_eq!(result.partial_reason.unwrap(), "TIER_LIMIT_EXCEEDED");
}

#[test]
fn test_policy_partial_approval_allocation() {
    let request = mock_request(6000); // requested 6000
    let access = mock_access(10000); // 50% allocation, big tier
    let treasury = mock_treasury(TreasuryHealth::Healthy);
    let constitution = mock_constitution();

    // wallet AUM = 10000, 50% allocation = 5000 max.
    // reservations = 0
    // headroom = 5000.

    let result = run_policy_engine(
        &request,
        &treasury,
        &access,
        &constitution,
        &BigDecimal::from(0),
        0,
        0,
    );

    assert!(result.approved);
    assert_eq!(result.approved_amount, BigDecimal::from(5000));
    assert_eq!(result.partial_reason.unwrap(), "AGENT_ALLOCATION_REACHED");
}
