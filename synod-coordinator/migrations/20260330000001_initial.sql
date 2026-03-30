-- Core Tables
CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE users (
  user_id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  email            VARCHAR(320) NOT NULL UNIQUE,
  password_hash    VARCHAR(255),  -- NULL if passkey-only
  created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_seen        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  is_active        BOOLEAN NOT NULL DEFAULT true
);

CREATE TABLE user_passkeys (
  passkey_id       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id          UUID NOT NULL REFERENCES users(user_id),
  credential_id    BYTEA NOT NULL UNIQUE,
  public_key       BYTEA NOT NULL,
  sign_count       BIGINT NOT NULL DEFAULT 0,
  device_type      VARCHAR(50),
  created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_used        TIMESTAMPTZ
);

CREATE TABLE wallet_connections (
  connection_id        UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id              UUID NOT NULL REFERENCES users(user_id),
  wallet_address       VARCHAR(56) NOT NULL UNIQUE,
  network              VARCHAR(20) NOT NULL,
  wc_session_topic     VARCHAR(255) NOT NULL,
  wc_session_expiry    TIMESTAMPTZ NOT NULL,
  ownership_sig        VARCHAR(256) NOT NULL,
  ownership_sig_hash   VARCHAR(64) NOT NULL,
  verified_at          TIMESTAMPTZ NOT NULL,
  status               VARCHAR(20) NOT NULL DEFAULT 'ACTIVE',
  connected_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  disconnected_at      TIMESTAMPTZ
);

CREATE TABLE treasuries (
  treasury_id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  owner_user_id        UUID NOT NULL REFERENCES users(user_id),
  name                 VARCHAR(255) NOT NULL,
  network              VARCHAR(20) NOT NULL,
  health               VARCHAR(20) NOT NULL DEFAULT 'PENDING_WALLET',
  peak_aum_usd         NUMERIC(20,7) NOT NULL DEFAULT 0,
  current_aum_usd      NUMERIC(20,7) NOT NULL DEFAULT 0,
  constitution_version INTEGER NOT NULL DEFAULT 0,
  created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  updated_at           TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE treasury_wallets (
  wallet_id        UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  treasury_id      UUID NOT NULL REFERENCES treasuries(treasury_id),
  wallet_address   VARCHAR(56) NOT NULL,
  label            VARCHAR(255),
  multisig_active  BOOLEAN NOT NULL DEFAULT false,
  status           VARCHAR(20) NOT NULL DEFAULT 'PENDING',
  added_at         TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE(treasury_id, wallet_address)
);

CREATE TABLE constitution_history (
  id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  treasury_id       UUID NOT NULL REFERENCES treasuries(treasury_id),
  version           INTEGER NOT NULL,
  constitution_hash VARCHAR(64) NOT NULL,
  constitution_json JSONB NOT NULL,
  governance_mode   VARCHAR(20) NOT NULL,
  updater_pubkey    VARCHAR(56) NOT NULL,
  rollback_from     INTEGER,
  created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE(treasury_id, version)
);

CREATE TABLE agent_slots (
  agent_id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  treasury_id      UUID NOT NULL REFERENCES treasuries(treasury_id),
  name             VARCHAR(255) NOT NULL,
  description      TEXT,
  api_key_hash     VARCHAR(255) NOT NULL UNIQUE,
  agent_pubkey     VARCHAR(56),
  status           VARCHAR(20) NOT NULL DEFAULT 'PENDING_CONNECTION',
  created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_connected   TIMESTAMPTZ,
  suspended_at     TIMESTAMPTZ,
  revoked_at       TIMESTAMPTZ
);

CREATE TABLE permit_groups (
  group_id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  agent_id             UUID NOT NULL REFERENCES agent_slots(agent_id),
  treasury_id          UUID NOT NULL REFERENCES treasuries(treasury_id),
  require_all          BOOLEAN NOT NULL DEFAULT true,
  status               VARCHAR(20) NOT NULL DEFAULT 'PENDING',
  total_requested_usd  NUMERIC(20,7) NOT NULL,
  total_approved_usd   NUMERIC(20,7) NOT NULL DEFAULT 0,
  total_pnl_usd        NUMERIC(20,7),
  created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at           TIMESTAMPTZ NOT NULL,
  consumed_at          TIMESTAMPTZ
);

CREATE TABLE permits (
  permit_id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  group_id             UUID NOT NULL REFERENCES permit_groups(group_id),
  leg_id               UUID NOT NULL,
  agent_id             UUID NOT NULL REFERENCES agent_slots(agent_id),
  treasury_id          UUID NOT NULL REFERENCES treasuries(treasury_id),
  wallet_address       VARCHAR(56) NOT NULL,
  pool_key             VARCHAR(100) NOT NULL,
  asset_code           VARCHAR(12) NOT NULL,
  asset_issuer         VARCHAR(56),
  requested_amount     NUMERIC(20,7) NOT NULL,
  approved_amount      NUMERIC(20,7) NOT NULL,
  status               VARCHAR(20) NOT NULL DEFAULT 'ACTIVE',
  deny_reason          VARCHAR(100),
  policy_check_number  INTEGER,
  tx_hash              VARCHAR(64),
  pnl_usd              NUMERIC(20,7),
  state_snapshot_hash  VARCHAR(64) NOT NULL,
  coordinator_sig      VARCHAR(256) NOT NULL,
  revoke_reason        VARCHAR(100),
  revoked_by           VARCHAR(56),
  created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at           TIMESTAMPTZ NOT NULL,
  consumed_at          TIMESTAMPTZ,
  UNIQUE(group_id, leg_id)
);

CREATE INDEX idx_permits_agent_status ON permits(agent_id, status);
CREATE INDEX idx_permits_treasury_status ON permits(treasury_id, status);
CREATE INDEX idx_permits_wallet_pool ON permits(wallet_address, pool_key, status);

CREATE TABLE events (
  event_id       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  treasury_id    UUID NOT NULL REFERENCES treasuries(treasury_id),
  event_type     VARCHAR(100) NOT NULL,
  sequence       BIGINT NOT NULL,
  payload        JSONB NOT NULL,
  emitted_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  UNIQUE(treasury_id, sequence)
);

CREATE INDEX idx_events_treasury_type ON events(treasury_id, event_type);
CREATE INDEX idx_events_treasury_seq ON events(treasury_id, sequence);

CREATE TABLE halt_log (
  id                    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  treasury_id           UUID NOT NULL REFERENCES treasuries(treasury_id),
  trigger_reason        VARCHAR(100) NOT NULL,
  drawdown_pct          NUMERIC(10,4),
  max_drawdown_pct      NUMERIC(10,4),
  peak_aum_usd          NUMERIC(20,7),
  current_aum_usd       NUMERIC(20,7),
  permits_revoked_count INTEGER NOT NULL DEFAULT 0,
  pools_locked_count    INTEGER NOT NULL DEFAULT 0,
  triggered_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  resumed_at            TIMESTAMPTZ,
  resumed_by            VARCHAR(56)
);
