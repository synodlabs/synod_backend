use synod_shared::models::*;
use synod_coordinator::policy::rules::*;
use bigdecimal::BigDecimal;
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
        current_aum_usd: BigDecimal::from(9500),
        state_hash: "hash".to_string(),
    }
}

fn mock_access(can_execute: bool, tier_limit: i64) -> AgentWalletAccess {
    AgentWalletAccess {
        agent_id: Uuid::new_v4(),
        wallet_address: "stellar_wallet".to_string(),
        allocation_pct: BigDecimal::from(50),
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
fn test_rule_04_agent_allocation() {
    let request = mock_request(5000); // requested 5000
    let access = mock_access(true, 10000); // 50% allocation
    
    // wallet AUM = 10000, 50% allocation = 5000 max.
    let wallet_aum = BigDecimal::from(10000);
    let active_res = BigDecimal::from(1000); // currently returning 1000 active
    // headroom = 5000 - 1000 = 4000.
    
    let approved = check_agent_allocation(&request, &access, &active_res, &wallet_aum).unwrap();
    assert_eq!(approved, BigDecimal::from(4000));
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
fn test_rule_10_drawdown_limit() {
    let mut state = mock_treasury(TreasuryHealth::Healthy);
    state.peak_aum_usd = BigDecimal::from(10000);
    state.current_aum_usd = BigDecimal::from(8000); // 20% drawdown
    
    let constitution = Constitution {
        treasury_id: Uuid::new_v4(),
        version: 1,
        memo: None,
        treasury_rules: TreasuryRules {
            max_drawdown_pct: BigDecimal::from(15),
            max_concurrent_permits: 10,
        },
        agent_wallet_rules: vec![],
    };
    
    assert!(check_drawdown_limit(&state, &constitution).is_err());
    
    state.current_aum_usd = BigDecimal::from(9000); // 10% drawdown
    assert!(check_drawdown_limit(&state, &constitution).is_ok());
}
