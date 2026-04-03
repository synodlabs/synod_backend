ALTER TABLE agent_slots 
ADD COLUMN fast_token_hash VARCHAR(64);

CREATE INDEX idx_agent_slots_fast_token_hash ON agent_slots(fast_token_hash);
