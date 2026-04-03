-- Drop the old pool-based index
DROP INDEX IF EXISTS idx_permits_wallet_pool;

-- Drop the pool_key column from permits
ALTER TABLE permits DROP COLUMN IF EXISTS pool_key;

-- Add agent_id based index to replace pool_key
CREATE INDEX idx_permits_wallet_agent ON permits(wallet_address, agent_id, status);

-- Add allocation_pct to agent_slots table
ALTER TABLE agent_slots ADD COLUMN IF NOT EXISTS allocation_pct NUMERIC(5,2) NOT NULL DEFAULT 100.0;
