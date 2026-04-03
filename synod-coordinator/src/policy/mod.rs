pub mod rules;

use synod_shared::models::*;
use bigdecimal::BigDecimal;
use self::rules::*;

pub fn run_policy_engine(
    request: &PermitRequest,
    treasury_state: &TreasuryState,
    agent_access: &AgentWalletAccess,
    constitution: &Constitution,
    total_active_reservations_usd: &BigDecimal,
    active_count: i32,
    treasury_active_count: i32,
) -> PolicyResult {
    // Start with requested amount as the baseline for approval
    let mut current_approved = request.requested_amount.clone();
    let mut partial_reason = None;

    // Rule 1: Treasury Halted
    if let Err(reason) = check_treasury_halted(treasury_state) {
        return deny(reason, 1);
    }

    // Rule 2: Agent Suspended
    if let Err(reason) = check_agent_suspended(agent_access) {
        return deny(reason, 2);
    }

    // Rule 3: Wallet Access
    if let Err(reason) = check_wallet_access(request, agent_access) {
        return deny(reason, 3);
    }

    // Rule 4: Agent Allocation (Supports Partial)
    match check_agent_allocation(request, agent_access, total_active_reservations_usd, &treasury_state.current_aum_usd) {
        Ok(limit) if limit < current_approved => {
            current_approved = limit;
            partial_reason = Some("AGENT_ALLOCATION_REACHED".to_string());
        }
        Ok(_) => {}
        Err(reason) => return deny(reason, 4),
    }

    // Rule 5: Tier Limit (Supports Partial)
    match check_tier_limit(request, agent_access) {
        Ok(limit) if limit < current_approved => {
            current_approved = limit;
            partial_reason = Some("TIER_LIMIT_EXCEEDED".to_string());
        }
        Ok(_) => {}
        Err(reason) => return deny(reason, 5),
    }

    // Rule 6: Concurrent Limit
    if let Err(reason) = check_concurrent_limit(active_count, agent_access) {
        return deny(reason, 6);
    }

    // Rule 7: Treasury Concurrent Limit
    if let Err(reason) = check_treasury_concurrent_limit(treasury_active_count, constitution) {
        return deny(reason, 7);
    }

    // Rule 8: Drawdown Limit
    if let Err(reason) = check_drawdown_limit(treasury_state, constitution) {
        return deny(reason, 8);
    }

    // Final Approval
    PolicyResult {
        approved: true,
        approved_amount: current_approved,
        deny_reason: None,
        policy_check_number: None,
        partial_reason,
    }
}

fn deny(reason: String, check_num: i32) -> PolicyResult {
    PolicyResult {
        approved: false,
        approved_amount: BigDecimal::from(0),
        deny_reason: Some(reason),
        policy_check_number: Some(check_num),
        partial_reason: None,
    }
}
