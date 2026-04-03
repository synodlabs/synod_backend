import asyncio
import base64
import json
import os
import tempfile
from unittest.mock import AsyncMock, patch

import pytest
import pytest_asyncio
from aioresponses import aioresponses
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from stellar_sdk import Keypair

from synod import SynodAgent
from synod.errors import (
    AgentSuspendedError,
    AllocationLimitError,
    PermitDeniedError,
    SynodConnectionError,
    WalletNotAssignedError,
)

# ── Helpers ──

@pytest.fixture
def mock_keys(tmp_path):
    """Provide a temporary directory for key storage and API key."""
    api_key = "synod_agent_test123"
    storage_path = str(tmp_path / "keys")
    return api_key, storage_path


@pytest.fixture
def base_url():
    return "http://localhost:8080"


# ── Initialization Tests ──

@pytest.mark.asyncio
async def test_init_generates_new_key(mock_keys):
    api_key, storage_path = mock_keys
    agent = SynodAgent(api_key, key_storage_path=storage_path)

    assert os.path.exists(os.path.join(storage_path, "synod_agent_key.enc"))
    assert os.path.exists(os.path.join(storage_path, "synod_agent_key.meta"))


@pytest.mark.asyncio
async def test_init_loads_existing_key(mock_keys):
    api_key, storage_path = mock_keys

    # First instance generates key
    agent1 = SynodAgent(api_key, key_storage_path=storage_path)
    pubkey1 = agent1._keypair.public_key

    # Second instance loads the same key
    agent2 = SynodAgent(api_key, key_storage_path=storage_path)
    pubkey2 = agent2._keypair.public_key

    assert pubkey1 == pubkey2


@pytest.mark.asyncio
async def test_init_existing_secret_key():
    kp = Keypair.random()
    api_key = "synod_agent_test123"

    agent = SynodAgent(api_key, existing_secret_key=kp.secret)
    assert agent._keypair.public_key == kp.public_key


# ── Connect Flow Tests ──

@pytest.mark.asyncio
async def test_connect_success_immediate(mock_keys, base_url):
    api_key, storage_path = mock_keys
    agent = SynodAgent(api_key, key_storage_path=storage_path, coordinator_url=base_url)

    with aioresponses() as m, patch("synod.ws.SynodWebSocket.start", new_callable=AsyncMock):
        m.post(
            f"{base_url}/v1/agents/connect",
            status=200,
            payload={
                "agent_id": "agent_1",
                "treasury_id": "treasury_1",
                "status": "ACTIVE",
                "wallet_access": [{"wallet_address": "G1", "agent_max_usd": "100"}],
                "coordinator_pubkey": "GC1",
            },
        )

        res = await agent.connect()
        assert res["status"] == "ACTIVE"
        assert res["agent_id"] == "agent_1"
        assert len(res["wallet_access"]) == 1

        # We also mock heartbeat start so it doesn't run forever in test
        assert agent._status == "ACTIVE"
        
        await agent.close()


@pytest.mark.asyncio
async def test_connect_polls_pending(mock_keys, base_url):
    api_key, storage_path = mock_keys
    agent = SynodAgent(api_key, key_storage_path=storage_path, coordinator_url=base_url)

    with aioresponses() as m, patch("synod.ws.SynodWebSocket.start", new_callable=AsyncMock), \
         patch("asyncio.sleep", new_callable=AsyncMock):
        
        # Connect returns PENDING_SIGNER
        m.post(
            f"{base_url}/v1/agents/connect",
            status=200,
            payload={
                "agent_id": "agent_1",
                "treasury_id": "treasury_1",
                "status": "PENDING_SIGNER",
            },
        )
        
        # First poll: still pending
        m.get(
            f"{base_url}/v1/agents/agent_1/status",
            status=200,
            payload={"status": "PENDING_SIGNER"},
        )

        # Second poll: active
        m.get(
            f"{base_url}/v1/agents/agent_1/status",
            status=200,
            payload={
                "status": "ACTIVE",
                "wallet_access": [{"wallet_address": "G1"}],
            },
        )

        res = await agent.connect()
        assert res["status"] == "ACTIVE"
        await agent.close()


@pytest.mark.asyncio
async def test_connect_handles_errors(mock_keys, base_url):
    api_key, storage_path = mock_keys
    agent = SynodAgent(api_key, key_storage_path=storage_path, coordinator_url=base_url)

    try:
        with aioresponses() as m:
            # Invalid API Key
            m.post(f"{base_url}/v1/agents/connect", status=401)
            with pytest.raises(SynodConnectionError, match="Invalid API key"):
                await agent.connect()

            # Suspended
            m.post(f"{base_url}/v1/agents/connect", status=403, payload={"error": "AGENT_SUSPENDED"})
            with pytest.raises(AgentSuspendedError):
                await agent.connect()

            # Pubkey conflict
            m.post(f"{base_url}/v1/agents/connect", status=409, payload={"error": "PUBKEY_CONFLICT"})
            with pytest.raises(SynodConnectionError, match="conflict"):
                await agent.connect()
    finally:
        await agent.close()


# ── Execute Flow Tests ──

@pytest_asyncio.fixture
async def connected_agent(mock_keys, base_url):
    api_key, storage_path = mock_keys
    agent = SynodAgent(api_key, key_storage_path=storage_path, coordinator_url=base_url)
    
    # Inject directly instead of connecting
    agent._agent_id = "agent_1"
    agent._treasury_id = "treasury_1"
    agent._status = "ACTIVE"
    agent._mock_source = Keypair.random().public_key
    agent._wallet_access = [
        {
            "wallet_address": agent._mock_source,
            "allocation_pct": 50.0,
            "agent_max_usd": "5000"
        }
    ]
    agent._session = __import__('aiohttp').ClientSession()
    
    yield agent
    
    await agent.close()


@pytest.mark.asyncio
async def test_execute_success(connected_agent, base_url):
    agent = connected_agent
    
    with aioresponses() as m, patch("synod.Server.load_account") as mock_load:
        # Mock stellar account
        mock_load.return_value.sequence = 1

        # 1. Permit request
        m.post(
            f"{base_url}/v1/permits/request",
            status=200,
            payload={
                "approved": True,
                "approved_amount": 1000.0,
                "permit_id": "permit_123",
            },
        )

        # 2. Co-sign request
        m.post(
            f"{base_url}/v1/permits/permit_123/cosign",
            status=200,
            payload={"tx_hash": "hash_123"},
        )

        # 3. Outcome
        m.post(
            f"{base_url}/v1/permits/permit_123/outcome",
            status=200,
            payload={},
        )

        result = await agent.execute(
            wallet=agent._mock_source,
            destination=Keypair.random().public_key,
            amount=1000.0,
            asset="XLM"
        )

        assert result.tx_hash == "hash_123"
        assert result.permit_id == "permit_123"
        assert result.approved_amount == 1000.0
        assert not result.partial

    await agent.close()


@pytest.mark.asyncio
async def test_execute_permit_denied(connected_agent, base_url):
    agent = connected_agent

    with aioresponses() as m:
        m.post(
            f"{base_url}/v1/permits/request",
            status=403,
            payload={"error": "ALLOCATION_LIMIT", "message": "Allocation limit exceeded"},
        )

        with pytest.raises(AllocationLimitError):
            await agent.execute(
                wallet=agent._mock_source,
                destination=Keypair.random().public_key,
                amount=99999.0,
                asset="XLM"
            )

    await agent.close()


@pytest.mark.asyncio
async def test_execute_wallet_not_assigned(connected_agent, base_url):
    agent = connected_agent

    with pytest.raises(WalletNotAssignedError):
        await agent.execute(
            wallet=Keypair.random().public_key,
            destination=Keypair.random().public_key,
            amount=1000.0,
            asset="XLM"
        )
    await agent.close()
