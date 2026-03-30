use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum SynodEvent {
    // Treasury lifecycle
    TreasuryCreated { treasury_id: Uuid },
    TreasuryHalted { treasury_id: Uuid, reason: String },
    TreasuryResumed { treasury_id: Uuid, resumed_by: String },

    // Wallet lifecycle
    WalletConnected { treasury_id: Uuid, wallet_address: String },
    WalletDisconnected { treasury_id: Uuid, wallet_address: String },
    MultisigEstablished { wallet_address: String },

    // Constitution
    ConstitutionUpdated { treasury_id: Uuid, version: i32 },

    // Agent lifecycle
    AgentConnected { agent_id: Uuid },
    AgentSuspended { agent_id: Uuid },
    AgentRevoked { agent_id: Uuid },

    // Permits
    PermitIssued { permit_id: Uuid, agent_id: Uuid },
    PermitDenied { agent_id: Uuid, reason: String },
    PermitConsumed { permit_id: Uuid },
    PermitExpired { permit_id: Uuid },
    PermitRevoked { permit_id: Uuid, reason: String },

    // Horizon
    InflowDetected { wallet_address: String, pool_key: String, amount: String },
    OutflowDetected { wallet_address: String, pool_key: String, amount: String },
    BalanceResyncComplete { treasury_id: Uuid },
    BalanceDiscrepancy { wallet_address: String, expected: String, actual: String },
    HorizonDegraded { wallet_address: String },

    // State
    PoolBalanceUpdate { treasury_id: Uuid, pool_key: String },
    StateUpdate { treasury_id: Uuid, state_hash: String },
    RebalanceOrderIssued { treasury_id: Uuid },
}
