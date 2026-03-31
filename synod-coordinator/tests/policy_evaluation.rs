use synod_shared::models::*;
use synod_coordinator::policy::run_policy_engine;
use bigdecimal::BigDecimal;
use uuid::Uuid;

fn mock_request(amount: i64) -> PermitRequest {
    PermitRequest {
        agent_id: Uuid::new_v4(),
        treasury_id: Uuid::new_v4(),
        wallet_address: "stellar_wallet".to_string(),
        pool_key: "pool:XLM".to_string(),
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
        pools: vec![
            PoolState {
                pool_key: "pool:XLM".to_string(),
                wallet_address: "stellar_wallet".to_string(),
                asset_code: "XLM".to_string(),
                balance_units: BigDecimal::from(5000),
                balance_usd: BigDecimal::from(5000),
                target_pct: BigDecimal::from(50),
                ceiling_pct: BigDecimal::from(60),
                floor_pct: BigDecimal::from(40),
                drift_threshold_pct: BigDecimal::from(2),
                locked: false,
            }
        ],
    }
}

fn mock_access(tier_limit: i64) -> AgentWalletAccess {
    AgentWalletAccess {
        agent_id: Uuid::new_v4(),
        wallet_address: "stellar_wallet".to_string(),
        pools: vec!["pool:XLM".to_string()],
        tier_limit_usd: BigDecimal::from(tier_limit),
        concurrent_permit_cap: 5,
        can_execute: true,
    }
}

fn mock_constitution() -> Constitution {
    Constitution {
        treasury_id: Uuid::new_v4(),
        version: 1,
        pools: vec![],
        max_drawdown_pct: BigDecimal::from(20),
        inflow_routing: vec![],
        governance_mode: "AUTO".to_string(),
    }
}

#[test]
fn test_policy_full_approval() {
    let request = mock_request(500);
    let treasury = mock_treasury(TreasuryHealth::Healthy);
    let access = mock_access(1000);
    let constitution = mock_constitution();
    let reservations = BigDecimal::from(0);
    
    let result = run_policy_engine(&request, &treasury, &access, &constitution, &reservations, 0);
    
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
    
    let result = run_policy_engine(&request, &treasury, &access, &constitution, &BigDecimal::from(0), 0);
    
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
    
    let result = run_policy_engine(&request, &treasury, &access, &constitution, &BigDecimal::from(0), 0);
    
    assert!(result.approved);
    assert_eq!(result.approved_amount, BigDecimal::from(1000));
    assert_eq!(result.partial_reason.unwrap(), "TIER_LIMIT_EXCEEDED");
}

#[test]
fn test_policy_partial_approval_ceiling() {
    let request = mock_request(1500);
    let access = mock_access(2000);
    let treasury = mock_treasury(TreasuryHealth::Healthy);
    
    // Pool balance 5000, Ceiling 60% of 10000 = 6000.
    // Headroom: 1000.
    let constitution = mock_constitution();
    
    let result = run_policy_engine(&request, &treasury, &access, &constitution, &BigDecimal::from(0), 0);
    
    assert!(result.approved);
    assert_eq!(result.approved_amount, BigDecimal::from(1000));
    assert_eq!(result.partial_reason.unwrap(), "POOL_CEILING_REACHED");
}

#[test]
fn test_policy_partial_approval_floor() {
    let request = mock_request(1500);
    let access = mock_access(2000);
    let mut treasury = mock_treasury(TreasuryHealth::Healthy);
    // Increase ceiling to 90% so it doesn't trigger partial
    treasury.pools[0].ceiling_pct = BigDecimal::from(90);
    
    // Pool balance 5000, Floor 40% of 10000 = 4000.
    // Spendable: 5000 - 4000 = 1000.
    let constitution = mock_constitution();
    
    let result = run_policy_engine(&request, &treasury, &access, &constitution, &BigDecimal::from(0), 0);
    
    assert!(result.approved);
    assert_eq!(result.approved_amount, BigDecimal::from(1000));
    assert_eq!(result.partial_reason.unwrap(), "POOL_FLOOR_VIOLATION");
}

#[test]
fn test_policy_min_of_all_limits() {
    let request = mock_request(3000);
    let access = mock_access(2500); // Tier limit 2500
    let treasury = mock_treasury(TreasuryHealth::Healthy);
    
    // Pool balance 5000, Ceiling 60% of 10000 = 6000. 
    // Headroom 1000. <-- This should be the final limit
    let constitution = mock_constitution();
    
    let result = run_policy_engine(&request, &treasury, &access, &constitution, &BigDecimal::from(0), 0);
    
    assert!(result.approved);
    assert_eq!(result.approved_amount, BigDecimal::from(1000));
}
