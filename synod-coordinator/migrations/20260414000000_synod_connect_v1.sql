ALTER TABLE agent_slots
  ALTER COLUMN api_key_hash DROP NOT NULL;

ALTER TABLE agent_slots
  ALTER COLUMN fast_token_hash DROP NOT NULL;

ALTER TABLE agent_slots
  ALTER COLUMN status TYPE VARCHAR(32);

CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_slots_agent_pubkey_unique
  ON agent_slots(agent_pubkey)
  WHERE agent_pubkey IS NOT NULL;
