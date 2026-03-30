// Redis key prefixes — single source of truth for all key patterns.
// Never hardcode Redis keys anywhere else in the codebase.

pub const TREASURY_STATE_PREFIX: &str = "treasury:state:";
pub const TREASURY_MUTEX_PREFIX: &str = "treasury:mutex:";
pub const CONSTITUTION_CACHE_PREFIX: &str = "treasury:constitution:";
pub const AGENT_STATUS_PREFIX: &str = "agent:status:";
pub const HORIZON_CURSOR_PREFIX: &str = "horizon:cursor:";
pub const HORIZON_SEEN_PREFIX: &str = "horizon:seen:";
pub const WC_SESSION_PREFIX: &str = "walletconnect:session:";
pub const OWNERSHIP_NONCE_PREFIX: &str = "wallet:nonce:";
pub const PASSKEY_CHALLENGE_PREFIX: &str = "auth:passkey:challenge:";
pub const RATE_LIMIT_PREFIX: &str = "auth:ratelimit:";

// Default TTLs (seconds)
pub const CURSOR_TTL_SECS: u64 = 604800;         // 7 days
pub const DEDUP_ENTRY_TTL_SECS: u64 = 172800;    // 48 hours
pub const DEDUP_SET_MAX_SIZE: usize = 2000;
pub const OWNERSHIP_NONCE_TTL_SECS: u64 = 600;   // 10 min
pub const PASSKEY_CHALLENGE_TTL_SECS: u64 = 300;  // 5 min
pub const TREASURY_MUTEX_TTL_SECS: u64 = 30;
pub const RATE_LIMIT_WINDOW_SECS: u64 = 900;     // 15 min
pub const RATE_LIMIT_MAX_ATTEMPTS: u64 = 10;

// Horizon
pub const HEARTBEAT_TIMEOUT_SECS: u64 = 30;
pub const SSE_RECONNECT_STAGGER_SECS: u64 = 2;

// Auth
pub const DEFAULT_JWT_EXPIRY_HOURS: u64 = 24;
pub const DEFAULT_BCRYPT_COST: u32 = 12;
