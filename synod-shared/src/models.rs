use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Treasury Health States ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TreasuryHealth {
    PendingWallet,
    PendingConstitution,
    Healthy,
    Halted,
    Degraded,
}

impl std::fmt::Display for TreasuryHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PendingWallet => write!(f, "PENDING_WALLET"),
            Self::PendingConstitution => write!(f, "PENDING_CONSTITUTION"),
            Self::Healthy => write!(f, "HEALTHY"),
            Self::Halted => write!(f, "HALTED"),
            Self::Degraded => write!(f, "DEGRADED"),
        }
    }
}

// ── Agent Status ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentStatus {
    PendingConnection,
    Active,
    Inactive,
    Suspended,
    Revoked,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PendingConnection => write!(f, "PENDING_CONNECTION"),
            Self::Active => write!(f, "ACTIVE"),
            Self::Inactive => write!(f, "INACTIVE"),
            Self::Suspended => write!(f, "SUSPENDED"),
            Self::Revoked => write!(f, "REVOKED"),
        }
    }
}

// ── Permit Status ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PermitStatus {
    Active,
    Denied,
    Consumed,
    Expired,
    Revoked,
    Failed,
}

impl std::fmt::Display for PermitStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "ACTIVE"),
            Self::Denied => write!(f, "DENIED"),
            Self::Consumed => write!(f, "CONSUMED"),
            Self::Expired => write!(f, "EXPIRED"),
            Self::Revoked => write!(f, "REVOKED"),
            Self::Failed => write!(f, "FAILED"),
        }
    }
}

// ── Permit Group Status ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PermitGroupStatus {
    Pending,
    Partial,
    Executing,
    Consumed,
    Expired,
    Revoked,
    Failed,
}

// ── Wallet Status ──

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WalletStatus {
    Pending,
    Active,
    Deactivated,
}

// ── Permit Request (input to policy engine) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermitRequest {
    pub agent_id: Uuid,
    pub treasury_id: Uuid,
    pub wallet_address: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub requested_amount: BigDecimal,
}

// ── Policy Result (output from policy engine) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyResult {
    pub approved: bool,
    pub approved_amount: BigDecimal,
    pub deny_reason: Option<String>,
    pub policy_check_number: Option<i32>,
    pub partial_reason: Option<String>,
}

// ── Treasury State (Redis snapshot) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreasuryState {
    pub treasury_id: Uuid,
    pub health: TreasuryHealth,
    pub peak_aum_usd: BigDecimal,
    pub current_aum_usd: BigDecimal,
    pub state_hash: String,
}



// ── Constitution ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constitution {
    pub treasury_id: Uuid,
    pub version: i32,
    pub memo: Option<String>,
    pub treasury_rules: TreasuryRules,
    pub agent_wallet_rules: Vec<AgentWalletRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstitutionContent {
    pub memo: Option<String>,
    pub treasury_rules: TreasuryRules,
    pub agent_wallet_rules: Vec<AgentWalletRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreasuryRules {
    pub max_drawdown_pct: BigDecimal,
    pub max_concurrent_permits: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentWalletRule {
    pub agent_id: Uuid,
    pub wallet_address: String,
    pub allocation_pct: BigDecimal,
    pub tier_limit_usd: BigDecimal,
    pub concurrent_permit_cap: i32,
}



// ── Agent Wallet Access (input to policy engine) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentWalletAccess {
    pub agent_id: Uuid,
    pub wallet_address: String,
    pub allocation_pct: BigDecimal,
    pub tier_limit_usd: BigDecimal,
    pub concurrent_permit_cap: i32,
    pub can_execute: bool,
}
