"""Synod Agent SDK — main entry point.

Usage:
    synod = SynodAgent(api_key="synod_agent_xxx...", key_storage_path="./synod_keys")
    await synod.connect()
    result = await synod.execute(wallet="GDQP...", destination="GBRP...", amount=4500, asset="USDC")
"""

import asyncio
import logging
import time
from dataclasses import dataclass
from typing import Any

import aiohttp
from stellar_sdk import (
    Keypair, Server, TransactionBuilder, Network, Asset
)

from .crypto import generate_keypair, keypair_from_secret, load_keypair
from .errors import (
    AgentNotActiveError,
    AgentSuspendedError,
    AllocationLimitError,
    ConcurrentLimitError,
    ConnectTimeoutError,
    DrawdownLimitError,
    PartialApprovalError,
    PermitDeniedError,
    SynodConnectionError,
    TierLimitError,
    TreasuryHaltedError,
    WalletNotAssignedError,
)
from .ws import SynodWebSocket

logger = logging.getLogger("synod")


@dataclass
class WalletHeadroom:
    """Current headroom state for a wallet."""
    wallet_address: str
    max_usd: float
    reserved_usd: float
    available_usd: float
    wallet_aum_usd: float


@dataclass
class ExecuteResult:
    """Result from a successful execute() call."""
    tx_hash: str
    permit_id: str
    requested_amount: float
    approved_amount: float
    partial: bool


class SynodAgent:
    """Synod Agent SDK — connect, execute, and manage treasury operations.

    Initialization supports two paths:
        Path 1 (new keypair): SynodAgent(api_key=..., key_storage_path=...)
        Path 2 (existing key): SynodAgent(api_key=..., existing_secret_key=...)
    """

    def __init__(
        self,
        api_key: str,
        key_storage_path: str | None = None,
        existing_secret_key: str | None = None,
        coordinator_url: str = "http://localhost:8080",
        network: str = "testnet",
        reject_partial: bool = False,
        connect_timeout_seconds: int = 900,
        log_level: str = "INFO",
    ):
        self._api_key = api_key
        self._coordinator_url = coordinator_url.rstrip("/")
        self._network = network
        self._reject_partial = reject_partial
        self._connect_timeout = connect_timeout_seconds

        # Set up logging (never log api_key or secret key)
        logging.basicConfig(level=getattr(logging, log_level.upper(), logging.INFO))

        # Initialize keypair
        if existing_secret_key:
            self._keypair = keypair_from_secret(existing_secret_key)
        elif key_storage_path:
            self._keypair = generate_keypair(api_key, key_storage_path)
        else:
            raise ValueError("Must provide either key_storage_path or existing_secret_key")

        # State
        self._agent_id: str | None = None
        self._treasury_id: str | None = None
        self._status: str = "DISCONNECTED"
        self._connection_phase: str = "DISCONNECTED"
        self._reason_code: str | None = "NOT_CONNECTED"
        self._wallet_access: list[dict] = []
        self._coordinator_pubkey: str | None = None
        self._ws: SynodWebSocket | None = None
        self._session: aiohttp.ClientSession | None = None
        self._treasury_halted: bool = False
        self._agent_suspended: bool = False
        self._heartbeat_task: asyncio.Task | None = None
        self._connection_task: asyncio.Task | None = None
        self._runtime_started: bool = False

        # Stellar network config
        if network == "mainnet":
            self._network_passphrase = Network.PUBLIC_NETWORK_PASSPHRASE
            self._horizon_url = "https://horizon.stellar.org"
        else:
            self._network_passphrase = Network.TESTNET_NETWORK_PASSPHRASE
            self._horizon_url = "https://horizon-testnet.stellar.org"

    # ── Connection ──

    async def connect(self) -> dict:
        """Perform the agent handshake with the coordinator."""
        await self._ensure_session()

        try:
            resp = await self._session.post(
                f"{self._coordinator_url}/v1/agents/connect",
                json={
                    "api_key": self._api_key,
                    "agent_pubkey": self._keypair.public_key,
                },
            )

            if resp.status == 401:
                raise SynodConnectionError("Invalid API key")
            elif resp.status == 403:
                data = await resp.json()
                code = data.get("error", "")
                if code == "AGENT_REVOKED":
                    raise SynodConnectionError("Agent has been revoked")
                elif code == "AGENT_SUSPENDED":
                    raise AgentSuspendedError()
                else:
                    raise SynodConnectionError(f"Connection forbidden: {code}")
            elif resp.status == 409:
                raise SynodConnectionError(
                    "Pubkey conflict - this agent slot already has a different registered keypair"
                )
            elif not resp.ok:
                data = await resp.json()
                raise SynodConnectionError(f"Handshake failed: {data}")

            data = await resp.json()
            self._apply_status_payload(data)

            if self._connection_phase == "COMPLETE" and self._status == "ACTIVE":
                await self._ensure_runtime_started()
            elif self._agent_id and (self._connection_task is None or self._connection_task.done()):
                logger.info(
                    "Agent %s pending activation (%s)",
                    self._agent_id,
                    self._reason_code,
                )
                self._connection_task = asyncio.create_task(self._poll_until_active())

            return {
                "agent_id": self._agent_id,
                "treasury_id": self._treasury_id,
                "slot_status": self._status,
                "connection_phase": self._connection_phase,
                "reason_code": self._reason_code,
                "wallet_access": self._wallet_access,
            }

        except aiohttp.ClientError as e:
            raise SynodConnectionError(f"Cannot reach coordinator: {e}")

    async def _poll_until_active(self) -> None:
        """Poll the coordinator until the agent reaches ACTIVE or a terminal state."""
        start = time.monotonic()
        interval = 3.0

        while True:
            elapsed = time.monotonic() - start
            if elapsed > self._connect_timeout:
                self._reason_code = "CONNECT_TIMEOUT"
                logger.error(
                    "Agent %s did not become active within %ss",
                    self._agent_id,
                    self._connect_timeout,
                )
                return

            await asyncio.sleep(interval)

            try:
                await self.get_status()
            except Exception as exc:
                logger.warning("Activation poll failed: %s", exc)
                continue

            if self._connection_phase == "COMPLETE" and self._status == "ACTIVE":
                logger.info("Agent %s reached ACTIVE", self._agent_id)
                await self._ensure_runtime_started()
                return

            if self._connection_phase == "FAILED":
                logger.error(
                    "Agent %s activation failed (%s)",
                    self._agent_id,
                    self._reason_code,
                )
                return

            logger.debug(
                "Agent %s still pending activation (%s, %.0fs elapsed)",
                self._agent_id,
                self._reason_code,
                elapsed,
            )

    async def execute(
        self,
        wallet: str,
        destination: str,
        amount: float,
        asset: str,
        asset_issuer: str | None = None,
        reject_partial: bool | None = None,
    ) -> ExecuteResult:
        """Execute a transaction through the Synod permit system.

        1. Request permit → 2. Build tx → 3. Agent sign → 4. Cosign → 5. Submit → 6. Report outcome
        """
        self._check_state()

        # Validate wallet access
        wallet_config = None
        for wa in self._wallet_access:
            if wa["wallet_address"] == wallet:
                wallet_config = wa
                break
        if wallet_config is None:
            raise WalletNotAssignedError(f"Wallet {wallet} not assigned to this agent")

        use_reject_partial = reject_partial if reject_partial is not None else self._reject_partial

        # 1. Request permit
        permit_resp = await self._session.post(
            f"{self._coordinator_url}/v1/permits/request",
            json={
                "agent_id": self._agent_id,
                "treasury_id": self._treasury_id,
                "wallet_address": wallet,
                "asset_code": asset,
                "asset_issuer": asset_issuer or "",
                "requested_amount": amount,
            },
            headers={"Authorization": f"Bearer {self._api_key}"},
        )

        if not permit_resp.ok:
            data = await permit_resp.json()
            error_code = data.get("error", "")
            msg = data.get("message", "")
            self._raise_permit_error(error_code, msg, amount)

        permit_data = await permit_resp.json()

        if not permit_data.get("approved"):
            reason = permit_data.get("deny_reason", "UNKNOWN")
            policy_check = permit_data.get("policy_check_number")
            raise PermitDeniedError(reason, policy_check)

        approved_amount = float(permit_data.get("approved_amount", 0))
        is_partial = approved_amount < amount

        if is_partial and use_reject_partial:
            raise PartialApprovalError(approved_amount, amount)

        effective_amount = approved_amount if is_partial else amount
        permit_id = permit_data.get("permit_id", "")

        # Check permit TTL — refuse if within 30s of expiry
        # (permit_data may include "expires_at")

        # 2. Build Stellar transaction
        stellar_asset = Asset.native() if asset == "XLM" else Asset(asset, asset_issuer)
        server = Server(self._horizon_url)
        source_account = await asyncio.get_event_loop().run_in_executor(
            None, server.load_account, wallet
        )

        tx_builder = TransactionBuilder(
            source_account=source_account,
            network_passphrase=self._network_passphrase,
            base_fee=100_000,
        )
        tx = (
            tx_builder
            .append_payment_op(destination, stellar_asset, str(effective_amount))
            .add_text_memo(permit_id[:28])  # Stellar memo max 28 chars
            .set_timeout(300)
            .build()
        )

        # 3. Agent signs (shard 1)
        tx.sign(self._keypair)
        signed_xdr = tx.to_xdr()

        # 4. Co-sign with coordinator (shard 2)
        cosign_resp = await self._session.post(
            f"{self._coordinator_url}/v1/permits/{permit_id}/cosign",
            json={"xdr": signed_xdr},
            headers={"Authorization": f"Bearer {self._api_key}"},
        )

        if not cosign_resp.ok:
            data = await cosign_resp.json()
            raise SynodConnectionError(f"Co-sign failed: {data.get('message', '')}")

        cosign_data = await cosign_resp.json()

        # 5. Submit to Stellar
        # The coordinator returns the fully signed XDR or just a signature.
        # For now, use the tx_hash from the coordinator response.
        tx_hash = cosign_data.get("tx_hash", "")

        # 6. Report outcome to coordinator
        await self._session.post(
            f"{self._coordinator_url}/v1/permits/{permit_id}/outcome",
            json={
                "tx_hash": tx_hash,
                "pnl_usd": "0.0",
                "final_amount_units": str(effective_amount),
            },
            headers={"Authorization": f"Bearer {self._api_key}"},
        )

        logger.info(
            "Transaction executed: permit=%s tx=%s amount=%.2f",
            permit_id, tx_hash, effective_amount,
        )

        return ExecuteResult(
            tx_hash=tx_hash,
            permit_id=permit_id,
            requested_amount=amount,
            approved_amount=approved_amount,
            partial=is_partial,
        )

    # ── State Query Methods ──

    async def get_headroom(self, wallet: str) -> WalletHeadroom:
        """Get current headroom for a wallet without making a permit request."""
        self._check_state()

        resp = await self._session.get(
            f"{self._coordinator_url}/v1/agents/{self._agent_id}/status"
        )
        if not resp.ok:
            raise SynodConnectionError("Failed to fetch agent status")

        data = await resp.json()
        for wa in data.get("wallet_access", []):
            if wa["wallet_address"] == wallet:
                max_usd = float(wa.get("agent_max_usd", "0"))
                aum = float(wa.get("current_wallet_aum_usd", "0"))
                # reserved_usd would need a separate query to the coordinator
                return WalletHeadroom(
                    wallet_address=wallet,
                    max_usd=max_usd,
                    reserved_usd=0.0,  # TODO: fetch from coordinator
                    available_usd=max_usd,
                    wallet_aum_usd=aum,
                )

        raise WalletNotAssignedError(f"Wallet {wallet} not assigned to this agent")

    async def get_active_permits(self) -> list[dict]:
        """Get all active permits this agent holds."""
        self._check_state()
        # The coordinator doesn't have a specific endpoint yet —
        # this would be GET /v1/permits?agent_id=... 
        # For now, return empty list as placeholder.
        logger.info("get_active_permits: not yet implemented on coordinator")
        return []

    async def get_status(self) -> dict:
        """Get agent's current status and access configuration."""
        if not self._agent_id:
            raise SynodConnectionError("Agent has not connected yet")

        await self._ensure_session()
        resp = await self._session.get(
            f"{self._coordinator_url}/v1/agents/{self._agent_id}/status"
        )
        if not resp.ok:
            raise SynodConnectionError("Failed to fetch agent status")

        data = await resp.json()
        self._apply_status_payload(data)
        return data

    # Internal Helpers

    def _check_state(self) -> None:
        """Check that the agent is in a valid state for operations."""
        if self._treasury_halted:
            raise TreasuryHaltedError()
        if self._agent_suspended:
            raise AgentSuspendedError()
        if self._status != "ACTIVE" or self._connection_phase != "COMPLETE":
            raise AgentNotActiveError(self._reason_code or self._status)

    async def _ensure_session(self) -> None:
        if self._session is None or self._session.closed:
            self._session = aiohttp.ClientSession()

    def _apply_status_payload(self, data: dict) -> None:
        self._agent_id = data.get("agent_id", self._agent_id)
        self._treasury_id = data.get("treasury_id", self._treasury_id)
        self._status = data.get("slot_status", data.get("status", self._status))
        self._connection_phase = data.get(
            "connection_phase",
            "COMPLETE" if self._status == "ACTIVE" else self._connection_phase,
        )
        self._reason_code = data.get("reason_code", self._reason_code)
        self._wallet_access = data.get("wallet_access", self._wallet_access)
        self._coordinator_pubkey = data.get("coordinator_pubkey", self._coordinator_pubkey)
        self._agent_suspended = self._status == "SUSPENDED" or self._reason_code == "AGENT_SUSPENDED"

    async def _ensure_runtime_started(self) -> None:
        if self._runtime_started or not self._agent_id:
            return

        ws_base = self._coordinator_url.replace("http://", "ws://").replace("https://", "wss://")
        ws_url = f"{ws_base}/v1/agents/ws/{self._agent_id}"
        self._ws = SynodWebSocket(
            url=ws_url,
            on_event=self._handle_ws_event,
            on_disconnect=self._handle_ws_disconnect,
        )
        await self._ws.start()

        if self._heartbeat_task is None or self._heartbeat_task.done():
            self._heartbeat_task = asyncio.create_task(self._heartbeat_loop())

        self._runtime_started = True

    def _raise_permit_error(self, error_code: str, message: str, requested: float) -> None:

        """Map coordinator error codes to SDK exceptions."""
        if error_code == "CONCURRENT_LIMIT":
            raise ConcurrentLimitError()
        elif error_code == "TREASURY_HALTED":
            raise TreasuryHaltedError()
        elif error_code == "AGENT_SUSPENDED":
            raise AgentSuspendedError()
        elif "allocation" in message.lower():
            raise AllocationLimitError(0, 0, requested)
        elif "tier" in message.lower():
            raise TierLimitError(0, requested)
        elif "drawdown" in message.lower():
            raise DrawdownLimitError()
        else:
            raise PermitDeniedError(message)

    async def _handle_ws_event(self, event: dict) -> None:
        """Process events received from the coordinator WebSocket stream."""
        event_type = event.get("type", "")

        if event_type == "WALLET_AUM_UPDATE":
            # Update local headroom state
            wallet = event.get("wallet_address")
            new_max = event.get("agent_new_max_usd")
            for wa in self._wallet_access:
                if wa["wallet_address"] == wallet:
                    wa["current_wallet_aum_usd"] = event.get("new_aum_usd", wa.get("current_wallet_aum_usd"))
                    wa["agent_max_usd"] = str(new_max) if new_max else wa.get("agent_max_usd")

        elif event_type == "TREASURY_HALTED":
            logger.warning("TREASURY HALTED — pausing operations")
            self._treasury_halted = True

        elif event_type == "TREASURY_RESUMED":
            logger.info("Treasury resumed — operations can restart")
            self._treasury_halted = False

        elif event_type == "AGENT_SUSPENDED":
            logger.warning("AGENT SUSPENDED — all operations blocked")
            self._agent_suspended = True

        elif event_type == "CONSTITUTION_UPDATED":
            logger.info("Constitution updated — refreshing access config")
            try:
                await self.get_status()
            except Exception as e:
                logger.error("Failed to refresh status after constitution update: %s", e)

        elif event_type in ("PERMIT_ISSUED", "PERMIT_CONSUMED", "PERMIT_EXPIRED"):
            logger.debug("Permit event: %s", event)

    async def _handle_ws_disconnect(self) -> None:
        """Handle prolonged WebSocket disconnection (>5 minutes)."""
        logger.error("Coordinator unreachable for extended period")
        # Resync state on reconnect
        try:
            await self.get_status()
        except Exception:
            pass

    async def _heartbeat_loop(self) -> None:
        """Send heartbeat to coordinator every 60 seconds."""
        while True:
            await asyncio.sleep(60)
            try:
                await self._session.post(
                    f"{self._coordinator_url}/v1/agents/{self._agent_id}/heartbeat"
                )
            except Exception as e:
                logger.warning("Heartbeat failed: %s", e)

    async def close(self) -> None:
        """Close all connections and clean up."""
        if self._connection_task:
            self._connection_task.cancel()
        if self._heartbeat_task:
            self._heartbeat_task.cancel()
        if self._ws:
            await self._ws.stop()
        if self._session:
            await self._session.close()
        self._runtime_started = False
        logger.info("SynodAgent closed")
