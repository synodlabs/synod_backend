-- Add policy columns to agent_slots
ALTER TABLE agent_slots ADD COLUMN tier_limit_usd NUMERIC(20,7) NOT NULL DEFAULT 1000;
ALTER TABLE agent_slots ADD COLUMN concurrent_permit_cap INTEGER NOT NULL DEFAULT 5;
ALTER TABLE agent_slots ADD COLUMN wallet_address VARCHAR(56);

-- Set some defaults
UPDATE agent_slots SET wallet_address = 'G...', tier_limit_usd = 5000, concurrent_permit_cap = 10;
