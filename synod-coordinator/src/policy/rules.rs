use synod_shared::models::*;
use bigdecimal::BigDecimal;

/// Rule 1: Check if the treasury is halted
pub fn check_treasury_halted(treasury_state: &TreasuryState) -> Result<(), String> {
    if treasury_state.health == TreasuryHealth::Halted {
        return Err("TREASURY_HALTED".to_string());
    }
    Ok(())
}

/// Rule 2: Check if the agent is suspended
pub fn check_agent_suspended(agent_access: &AgentWalletAccess) -> Result<(), String> {
    if !agent_access.can_execute {
        return Err("AGENT_SUSPENDED".to_string());
    }
    Ok(())
}

/// Rule 3: Check if the agent has access to the requested wallet
pub fn check_wallet_access(request: &PermitRequest, agent_access: &AgentWalletAccess) -> Result<(), String> {
    if request.wallet_address != agent_access.wallet_address {
        return Err("WALLET_ACCESS_DENIED".to_string());
    }
    Ok(())
}

/// Rule 4: Check Agent Allocation Limits
pub fn check_agent_allocation(
    request: &PermitRequest,
    agent_access: &AgentWalletAccess,
    total_active_reservations_usd: &BigDecimal,
    wallet_aum_usd: &BigDecimal
) -> Result<BigDecimal, String> {
    let allocation_ratio = &agent_access.allocation_pct / BigDecimal::from(100);
    let max_allowed_usd = allocation_ratio * wallet_aum_usd;
    
    let current_total_usd = total_active_reservations_usd.clone();
    
    if current_total_usd >= max_allowed_usd {
        return Err("AGENT_ALLOCATION_REACHED".to_string());
    }
    
    let headroom = max_allowed_usd - current_total_usd;
    if request.requested_amount > headroom {
        return Ok(headroom);
    }
    
    Ok(request.requested_amount.clone())
}
/// Rule 6: Check Agent Tier Limit
pub fn check_tier_limit(request: &PermitRequest, agent_access: &AgentWalletAccess) -> Result<BigDecimal, String> {
    if request.requested_amount > agent_access.tier_limit_usd {
        return Ok(agent_access.tier_limit_usd.clone());
    }
    Ok(request.requested_amount.clone())
}

/// Rule 7: Check Concurrent Permit Cap
pub fn check_concurrent_limit(active_count: i32, agent_access: &AgentWalletAccess) -> Result<(), String> {
    if active_count >= agent_access.concurrent_permit_cap {
        return Err("CONCURRENT_LIMIT_REACHED".to_string());
    }
    Ok(())
}

pub fn check_treasury_concurrent_limit(active_count: i32, constitution: &Constitution) -> Result<(), String> {
    if active_count >= constitution.treasury_rules.max_concurrent_permits {
        return Err("TREASURY_CONCURRENT_LIMIT_REACHED".to_string());
    }
    Ok(())
}

/// Rule 8: Check Drawdown Limit
pub fn check_drawdown_limit(treasury_state: &TreasuryState, constitution: &Constitution) -> Result<(), String> {
    if treasury_state.peak_aum_usd <= BigDecimal::from(0) {
        return Ok(());
    }
    
    let drawdown_pct = (&treasury_state.peak_aum_usd - &treasury_state.current_aum_usd) 
        / &treasury_state.peak_aum_usd * BigDecimal::from(100);
    
    if drawdown_pct >= constitution.treasury_rules.max_drawdown_pct {
        return Err("MAX_DRAWDOWN_REACHED".to_string());
    }
    
    Ok(())
}
