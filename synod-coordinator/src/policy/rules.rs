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

/// Rule 4: Check if the agent has access to the requested pool
pub fn check_pool_access(request: &PermitRequest, agent_access: &AgentWalletAccess) -> Result<(), String> {
    if !agent_access.pools.contains(&request.pool_key) {
        return Err("POOL_ACCESS_DENIED".to_string());
    }
    Ok(())
}

/// Rule 5: Check if the pool is manually locked
pub fn check_pool_locked(request: &PermitRequest, treasury_state: &TreasuryState) -> Result<(), String> {
    if let Some(pool) = treasury_state.pools.iter().find(|p| p.pool_key == request.pool_key) {
        if pool.locked {
            return Err("POOL_LOCKED".to_string());
        }
    }
    Ok(())
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

/// Rule 8: Check Pool Ceiling
pub fn check_pool_ceiling(
    request: &PermitRequest,
    pool_state: &PoolState,
    total_active_reservations_usd: &BigDecimal,
    wallet_aum_usd: &BigDecimal
) -> Result<BigDecimal, String> {
    // headroom_usd = (ceiling_pct × wallet_aum_usd) - current_pool_balance_usd - sum(ACTIVE permit reservations)
    let ceiling_ratio = &pool_state.ceiling_pct / BigDecimal::from(100);
    let max_pool_usd = ceiling_ratio * wallet_aum_usd;
    
    let current_total_usd = &pool_state.balance_usd + total_active_reservations_usd;
    
    if current_total_usd >= max_pool_usd {
        return Err("POOL_CEILING_REACHED".to_string());
    }
    
    let headroom = max_pool_usd - current_total_usd;
    if request.requested_amount > headroom {
        return Ok(headroom);
    }
    
    Ok(request.requested_amount.clone())
}

/// Rule 9: Check Pool Floor
pub fn check_pool_floor(
    request: &PermitRequest,
    pool_state: &PoolState,
    total_active_reservations_usd: &BigDecimal,
    wallet_aum_usd: &BigDecimal
) -> Result<BigDecimal, String> {
    // floor_usd = floor_pct × wallet_aum_usd
    // safe_to_spend = current_pool_balance_usd - floor_usd - sum(ACTIVE permit reservations)
    let floor_ratio = &pool_state.floor_pct / BigDecimal::from(100);
    let floor_usd = floor_ratio * wallet_aum_usd;
    
    let spendable_before_request = &pool_state.balance_usd - &floor_usd - total_active_reservations_usd;
    
    if spendable_before_request <= BigDecimal::from(0) {
        return Err("POOL_FLOOR_VIOLATION".to_string());
    }
    
    if request.requested_amount > spendable_before_request {
        return Ok(spendable_before_request);
    }
    
    Ok(request.requested_amount.clone())
}

/// Rule 10: Check Drawdown Limit
pub fn check_drawdown_limit(treasury_state: &TreasuryState, constitution: &Constitution) -> Result<(), String> {
    if treasury_state.peak_aum_usd <= BigDecimal::from(0) {
        return Ok(());
    }
    
    let drawdown_pct = (&treasury_state.peak_aum_usd - &treasury_state.current_aum_usd) 
        / &treasury_state.peak_aum_usd * BigDecimal::from(100);
    
    if drawdown_pct >= constitution.max_drawdown_pct {
        return Err("MAX_DRAWDOWN_REACHED".to_string());
    }
    
    Ok(())
}
