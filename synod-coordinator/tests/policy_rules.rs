use synod_shared::models::*;
use synod_coordinator::policy::rules::*;
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
        current_aum_usd: BigDecimal::from(9500),
        state_hash: "hash".to_string(),
        pools: vec![
            PoolState {
                pool_key: "pool:XLM".to_string(),
                wallet_address: "stellar_wallet".to_string(),
                asset_code: "XLM".to_string(),
                balance_units: BigDecimal::from(1000),
                balance_usd: BigDecimal::from(1000),
                target_pct: BigDecimal::from(10),
                ceiling_pct: BigDecimal::from(20),
                floor_pct: BigDecimal::from(5),
                drift_threshold_pct: BigDecimal::from(2),
                locked: false,
            }
        ],
    }
}

fn mock_access(can_execute: bool, tier_limit: i64) -> AgentWalletAccess {
    AgentWalletAccess {
        agent_id: Uuid::new_v4(),
        wallet_address: "stellar_wallet".to_string(),
        pools: vec!["pool:XLM".to_string()],
        tier_limit_usd: BigDecimal::from(tier_limit),
        concurrent_permit_cap: 5,
        can_execute,
    }
}

#[test]
fn test_rule_01_treasury_halted() {
    let state = mock_treasury(TreasuryHealth::Halted);
    assert!(check_treasury_halted(&state).is_err());
    
    let healthy = mock_treasury(TreasuryHealth::Healthy);
    assert!(check_treasury_halted(&healthy).is_ok());
}

#[test]
fn test_rule_02_agent_suspended() {
    let access = mock_access(false, 1000);
    assert!(check_agent_suspended(&access).is_err());
    
    let active = mock_access(true, 1000);
    assert!(check_agent_suspended(&active).is_ok());
}

#[test]
fn test_rule_03_wallet_access() {
    let mut request = mock_request(100);
    let access = mock_access(true, 1000);
    
    assert!(check_wallet_access(&request, &access).is_ok());
    
    request.wallet_address = "different_wallet".to_string();
    assert!(check_wallet_access(&request, &access).is_err());
}

#[test]
fn test_rule_04_pool_access() {
    let mut request = mock_request(100);
    let access = mock_access(true, 1000);
    
    assert!(check_pool_access(&request, &access).is_ok());
    
    request.pool_key = "unauthorized_pool".to_string();
    assert!(check_pool_access(&request, &access).is_err());
}

#[test]
fn test_rule_05_pool_locked() {
    let request = mock_request(100);
    let mut state = mock_treasury(TreasuryHealth::Healthy);
    
    assert!(check_pool_locked(&request, &state).is_ok());
    
    state.pools[0].locked = true;
    assert!(check_pool_locked(&request, &state).is_err());
}

#[test]
fn test_rule_06_tier_limit() {
    let request = mock_request(1500);
    let access = mock_access(true, 1000);
    
    let approved = check_tier_limit(&request, &access).unwrap();
    assert_eq!(approved, BigDecimal::from(1000));
}

#[test]
fn test_rule_07_concurrent_limit() {
    let access = mock_access(true, 1000);
    
    assert!(check_concurrent_limit(4, &access).is_ok());
    assert!(check_concurrent_limit(5, &access).is_err());
}

#[test]
fn test_rule_08_pool_ceiling() {
    let request = mock_request(2000);
    let state = mock_treasury(TreasuryHealth::Healthy);
    let pool = &state.pools[0]; // balance 1000, ceiling 20% of 10000 (Wait, I used 9500 in previous mock, let's use 10000 now) = 2000
    
    // Total used: balance (1000) + active (200) = 1200
    // Ceiling: 2000. Headroom: 800.
    let active_res = BigDecimal::from(200);
    let approved = check_pool_ceiling(&request, pool, &active_res, &state.peak_aum_usd).unwrap();
    assert_eq!(approved, BigDecimal::from(800));
}

#[test]
fn test_rule_09_pool_floor() {
    let request = mock_request(1000);
    let state = mock_treasury(TreasuryHealth::Healthy);
    let pool = &state.pools[0]; // balance 1000, floor 5% of 10000 = 500
    
    // Spendable: balance (1000) - floor (500) - active (100) = 400
    let active_res = BigDecimal::from(100);
    let approved = check_pool_floor(&request, pool, &active_res, &state.peak_aum_usd).unwrap();
    assert_eq!(approved, BigDecimal::from(400));
}

#[test]
fn test_rule_10_drawdown_limit() {
    let mut state = mock_treasury(TreasuryHealth::Healthy);
    state.peak_aum_usd = BigDecimal::from(10000);
    state.current_aum_usd = BigDecimal::from(8000); // 20% drawdown
    
    let constitution = Constitution {
        treasury_id: Uuid::new_v4(),
        version: 1,
        pools: vec![],
        max_drawdown_pct: BigDecimal::from(15),
        inflow_routing: vec![],
        governance_mode: "AUTO".to_string(),
    };
    
    assert!(check_drawdown_limit(&state, &constitution).is_err());
    
    state.current_aum_usd = BigDecimal::from(9000); // 10% drawdown
    assert!(check_drawdown_limit(&state, &constitution).is_ok());
}
