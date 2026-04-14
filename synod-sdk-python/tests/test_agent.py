import os
from datetime import datetime, timedelta, timezone
from unittest.mock import AsyncMock, patch

import aiohttp
import pytest
import pytest_asyncio
from aioresponses import aioresponses
from stellar_sdk import Keypair
from stellar_sdk.account import Account

from synod import SynodAgent
from synod.errors import (
    AgentSuspendedError,
    SynodConnectionError,
    WalletNotAssignedError,
)


@pytest.fixture
def storage_path(tmp_path):
    return str(tmp_path / "keys")


@pytest.fixture
def base_url():
    return "http://localhost:8080"


def iso_in(seconds: int) -> str:
    return (datetime.now(timezone.utc) + timedelta(seconds=seconds)).isoformat()


@pytest.mark.asyncio
async def test_init_generates_new_key(storage_path):
    SynodAgent(key_storage_path=storage_path)
    assert os.path.exists(os.path.join(storage_path, "synod_agent_key.json"))


@pytest.mark.asyncio
async def test_init_loads_existing_key(storage_path):
    first = SynodAgent(key_storage_path=storage_path)
    second = SynodAgent(key_storage_path=storage_path)
    assert first.public_key == second.public_key


@pytest.mark.asyncio
async def test_init_existing_secret_key():
    kp = Keypair.random()
    agent = SynodAgent(existing_secret_key=kp.secret)
    assert agent.public_key == kp.public_key


@pytest.mark.asyncio
async def test_connect_success_immediate(storage_path, base_url):
    agent = SynodAgent(key_storage_path=storage_path, coordinator_url=base_url)

    with aioresponses() as mocked, patch("synod.ws.SynodWebSocket.start", new_callable=AsyncMock):
        mocked.post(
            f"{base_url}/v1/agents/connect/challenge",
            status=200,
            payload={
                "agent_id": "agent_1",
                "treasury_id": "treasury_1",
                "challenge": "challenge_1",
                "expires_at": iso_in(300),
            },
        )
        mocked.post(
            f"{base_url}/v1/agents/connect/complete",
            status=200,
            payload={
                "agent_id": "agent_1",
                "treasury_id": "treasury_1",
                "slot_status": "ACTIVE",
                "connection_phase": "COMPLETE",
                "reason_code": None,
                "wallet_access": [{"wallet_address": "G1", "agent_max_usd": "1000.00", "current_wallet_aum_usd": "5000.00"}],
                "websocket_endpoint": "/v1/agents/ws/agent_1",
                "websocket_token": "ws_token",
                "session_token": "session_token",
                "expires_at": iso_in(3600),
                "coordinator_pubkey": "GC1",
            },
        )

        data = await agent.connect()
        assert data["slot_status"] == "ACTIVE"
        assert data["connection_phase"] == "COMPLETE"
        assert data["public_key"] == agent.public_key
        assert agent._session_token == "session_token"
        assert agent._websocket_token == "ws_token"
        assert agent._runtime_started is True

    await agent.close()


@pytest.mark.asyncio
async def test_connect_requires_enrollment(storage_path, base_url):
    agent = SynodAgent(key_storage_path=storage_path, coordinator_url=base_url)

    with aioresponses() as mocked:
        mocked.post(f"{base_url}/v1/agents/connect/challenge", status=404)
        with pytest.raises(SynodConnectionError, match="not enrolled"):
            await agent.connect()

    await agent.close()


@pytest_asyncio.fixture
async def connected_agent(storage_path, base_url):
    agent = SynodAgent(key_storage_path=storage_path, coordinator_url=base_url)
    agent._agent_id = "agent_1"
    agent._treasury_id = "treasury_1"
    agent._status = "ACTIVE"
    agent._connection_phase = "COMPLETE"
    agent._session_token = "session_token"
    agent._websocket_token = "ws_token"
    agent._ticket_expires_at = iso_in(3600)
    agent._wallet_access = [
        {
            "wallet_address": Keypair.random().public_key,
            "allocation_pct": 50.0,
            "agent_max_usd": "5000.00",
            "current_wallet_aum_usd": "10000.00",
        }
    ]
    agent._session = aiohttp.ClientSession()
    yield agent
    await agent.close()


@pytest.mark.asyncio
async def test_get_status_refreshes_ticket_when_expiring(connected_agent, base_url):
    connected_agent._ticket_expires_at = iso_in(10)

    with aioresponses() as mocked:
        mocked.post(
            f"{base_url}/v1/agents/ws-ticket/refresh",
            status=200,
            payload={
                "session_token": "session_token_2",
                "websocket_token": "ws_token_2",
                "expires_at": iso_in(3600),
            },
        )
        mocked.get(
            f"{base_url}/v1/agents/{connected_agent._agent_id}/status",
            status=200,
            payload={
                "agent_id": connected_agent._agent_id,
                "treasury_id": connected_agent._treasury_id,
                "slot_status": "ACTIVE",
                "connection_phase": "COMPLETE",
                "reason_code": None,
                "wallet_access": connected_agent._wallet_access,
            },
        )

        data = await connected_agent.get_status()
        assert data["slot_status"] == "ACTIVE"
        assert connected_agent._session_token == "session_token_2"
        assert connected_agent._websocket_token == "ws_token_2"


@pytest.mark.asyncio
async def test_execute_success(connected_agent, base_url):
    wallet = connected_agent._wallet_access[0]["wallet_address"]

    with aioresponses() as mocked, patch("synod.Server.load_account") as mock_load_account:
        mock_load_account.return_value = Account(wallet, 1)

        mocked.post(
            f"{base_url}/v1/permits/request",
            status=201,
            payload={
                "permit_id": "permit_123",
                "approved": True,
                "approved_amount": 250.0,
                "deny_reason": None,
                "policy_check_number": 7,
                "partial_reason": None,
            },
        )
        mocked.post(
            f"{base_url}/v1/permits/permit_123/cosign",
            status=200,
            payload={"status": "SIGNED", "tx_hash": "hash_123"},
        )
        mocked.post(
            f"{base_url}/v1/permits/permit_123/outcome",
            status=200,
            payload={},
        )

        result = await connected_agent.execute(
            wallet=wallet,
            destination=Keypair.random().public_key,
            amount=250.0,
            asset="XLM",
        )

        assert result.tx_hash == "hash_123"
        assert result.permit_id == "permit_123"
        assert result.approved_amount == 250.0
        assert result.partial is False


@pytest.mark.asyncio
async def test_execute_wallet_not_assigned(connected_agent):
    with pytest.raises(WalletNotAssignedError):
        await connected_agent.execute(
            wallet=Keypair.random().public_key,
            destination=Keypair.random().public_key,
            amount=100.0,
            asset="XLM",
        )


@pytest.mark.asyncio
async def test_connect_maps_suspension_error(storage_path, base_url):
    agent = SynodAgent(key_storage_path=storage_path, coordinator_url=base_url)

    with aioresponses() as mocked:
        mocked.post(
            f"{base_url}/v1/agents/connect/challenge",
            status=403,
            payload={"error": "AGENT_SUSPENDED"},
        )
        with pytest.raises(AgentSuspendedError):
            await agent.connect()

    await agent.close()
