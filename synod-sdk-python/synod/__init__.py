"""Synod Agent SDK for the Synod Connect public-key enrollment flow."""

from __future__ import annotations

import asyncio
import logging
import time
from dataclasses import dataclass
from datetime import datetime, timezone

import aiohttp
from stellar_sdk import Asset, Keypair, Network, Server, TransactionBuilder

from .crypto import (
    build_signed_request_auth,
    generate_keypair,
    keypair_from_secret,
    sign_stellar_message,
)
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
    wallet_address: str
    max_usd: float
    reserved_usd: float
    available_usd: float
    wallet_aum_usd: float


@dataclass
class ExecuteResult:
    tx_hash: str
    permit_id: str
    requested_amount: float
    approved_amount: float
    partial: bool


class SynodAgent:
    """Synod Connect SDK client using one local agent keypair."""

    def __init__(
        self,
        key_storage_path: str | None = None,
        existing_secret_key: str | None = None,
        coordinator_url: str = "http://localhost:8080",
        network: str = "testnet",
        reject_partial: bool = False,
        connect_timeout_seconds: int = 900,
        log_level: str = "INFO",
    ):
        self._coordinator_url = coordinator_url.rstrip("/")
        self._network = network
        self._reject_partial = reject_partial
        self._connect_timeout = connect_timeout_seconds

        logging.basicConfig(level=getattr(logging, log_level.upper(), logging.INFO))

        if existing_secret_key:
            self._keypair = keypair_from_secret(existing_secret_key)
        elif key_storage_path:
            self._keypair = generate_keypair(key_storage_path)
        else:
            raise ValueError("Must provide either key_storage_path or existing_secret_key")

        self._agent_id: str | None = None
        self._treasury_id: str | None = None
        self._status: str = "DISCONNECTED"
        self._connection_phase: str = "DISCONNECTED"
        self._reason_code: str | None = "NOT_CONNECTED"
        self._wallet_access: list[dict] = []
        self._coordinator_pubkey: str | None = None
        self._session_token: str | None = None
        self._websocket_token: str | None = None
        self._ticket_expires_at: str | None = None
        self._ws: SynodWebSocket | None = None
        self._session: aiohttp.ClientSession | None = None
        self._treasury_halted: bool = False
        self._agent_suspended: bool = False
        self._heartbeat_task: asyncio.Task | None = None
        self._connection_task: asyncio.Task | None = None
        self._runtime_started: bool = False

        if network == "mainnet":
            self._network_passphrase = Network.PUBLIC_NETWORK_PASSPHRASE
            self._horizon_url = "https://horizon.stellar.org"
        else:
            self._network_passphrase = Network.TESTNET_NETWORK_PASSPHRASE
            self._horizon_url = "https://horizon-testnet.stellar.org"

    @property
    def public_key(self) -> str:
        return self._keypair.public_key

    async def connect(self) -> dict:
        await self._ensure_session()

        try:
            challenge_resp = await self._session.post(
                f"{self._coordinator_url}/v1/agents/connect/challenge",
                json={"agent_pubkey": self.public_key},
            )

            if challenge_resp.status == 404:
                raise SynodConnectionError(
                    "Agent public key is not enrolled in Synod yet. Paste this key into the dashboard slot first."
                )
            if challenge_resp.status == 403:
                data = await challenge_resp.json()
                code = data.get("error", "")
                if code == "AGENT_SUSPENDED":
                    raise AgentSuspendedError()
                raise SynodConnectionError(f"Connection forbidden: {code}")
            if not challenge_resp.ok:
                data = await challenge_resp.json()
                raise SynodConnectionError(f"Challenge request failed: {data}")

            challenge_data = await challenge_resp.json()
            signed_message = sign_stellar_message(
                self._keypair,
                f"synod-connect:{challenge_data['agent_id']}:{challenge_data['treasury_id']}:{self.public_key}:{challenge_data['challenge']}",
            )

            complete_resp = await self._session.post(
                f"{self._coordinator_url}/v1/agents/connect/complete",
                json={
                    "agent_pubkey": self.public_key,
                    "challenge": challenge_data["challenge"],
                    "signature": signed_message,
                },
            )

            if not complete_resp.ok:
                data = await complete_resp.json()
                raise SynodConnectionError(f"Connection completion failed: {data}")

            data = await complete_resp.json()
            self._apply_status_payload(data)
            self._session_token = data.get("session_token")
            self._websocket_token = data.get("websocket_token")
            self._ticket_expires_at = data.get("expires_at")

            if self._connection_phase == "COMPLETE" and self._status == "ACTIVE":
                await self._ensure_runtime_started()
            elif self._agent_id and (self._connection_task is None or self._connection_task.done()):
                logger.info("Agent %s pending activation (%s)", self._agent_id, self._reason_code)
                self._connection_task = asyncio.create_task(self._poll_until_active())

            return {
                "agent_id": self._agent_id,
                "treasury_id": self._treasury_id,
                "slot_status": self._status,
                "connection_phase": self._connection_phase,
                "reason_code": self._reason_code,
                "wallet_access": self._wallet_access,
                "public_key": self.public_key,
                "expires_at": self._ticket_expires_at,
            }
        except aiohttp.ClientError as exc:
            raise SynodConnectionError(f"Cannot reach coordinator: {exc}") from exc

    async def get_status(self) -> dict:
        if not self._agent_id:
            raise SynodConnectionError("Agent has not connected yet")

        await self._ensure_session()
        await self._ensure_runtime_ticket()
        resp = await self._session.get(
            f"{self._coordinator_url}/v1/agents/{self._agent_id}/status",
            headers=self._auth_headers(),
        )
        if not resp.ok:
            raise SynodConnectionError("Failed to fetch agent status")

        data = await resp.json()
        self._apply_status_payload(data)
        return data

    async def execute(
        self,
        wallet: str,
        destination: str,
        amount: float,
        asset: str,
        asset_issuer: str | None = None,
        reject_partial: bool | None = None,
    ) -> ExecuteResult:
        self._check_state()
        await self._ensure_session()
        await self._ensure_runtime_ticket()

        wallet_config = next((wa for wa in self._wallet_access if wa["wallet_address"] == wallet), None)
        if wallet_config is None:
            raise WalletNotAssignedError(f"Wallet {wallet} not assigned to this agent")

        use_reject_partial = reject_partial if reject_partial is not None else self._reject_partial
        permit_payload = {
            "agent_id": self._agent_id,
            "treasury_id": self._treasury_id,
            "wallet_address": wallet,
            "asset_code": asset,
            "asset_issuer": asset_issuer or "",
            "requested_amount": amount,
        }
        permit_body = {
            **permit_payload,
            "request_auth": self._signed_auth("permit.request", permit_payload),
        }

        permit_resp = await self._session.post(
            f"{self._coordinator_url}/v1/permits/request",
            json=permit_body,
            headers=self._auth_headers(),
        )
        if not permit_resp.ok:
            data = await permit_resp.json()
            self._raise_permit_error(data.get("error", ""), data.get("message", ""), amount)

        permit_data = await permit_resp.json()
        if not permit_data.get("approved"):
            raise PermitDeniedError(permit_data.get("deny_reason", "UNKNOWN"), permit_data.get("policy_check_number"))

        approved_amount = float(permit_data.get("approved_amount", 0))
        is_partial = approved_amount < amount
        if is_partial and use_reject_partial:
            raise PartialApprovalError(approved_amount, amount)

        effective_amount = approved_amount if is_partial else amount
        permit_id = permit_data.get("permit_id", "")

        stellar_asset = Asset.native() if asset == "XLM" else Asset(asset, asset_issuer)
        server = Server(self._horizon_url)
        source_account = await asyncio.get_event_loop().run_in_executor(None, server.load_account, wallet)

        tx = (
            TransactionBuilder(
                source_account=source_account,
                network_passphrase=self._network_passphrase,
                base_fee=100_000,
            )
            .append_payment_op(destination, stellar_asset, str(effective_amount))
            .add_text_memo(permit_id[:28])
            .set_timeout(300)
            .build()
        )
        tx.sign(self._keypair)
        signed_xdr = tx.to_xdr()

        cosign_payload = {"xdr": signed_xdr}
        cosign_resp = await self._session.post(
            f"{self._coordinator_url}/v1/permits/{permit_id}/cosign",
            json={
                **cosign_payload,
                "request_auth": self._signed_auth("permit.cosign", signed_xdr),
            },
            headers=self._auth_headers(),
        )
        if not cosign_resp.ok:
            data = await cosign_resp.json()
            raise SynodConnectionError(f"Co-sign failed: {data.get('message', '')}")

        cosign_data = await cosign_resp.json()
        tx_hash = cosign_data.get("tx_hash", "")

        outcome_payload = {
            "tx_hash": tx_hash,
            "pnl_usd": "0.0",
            "final_amount_units": str(effective_amount),
        }
        await self._session.post(
            f"{self._coordinator_url}/v1/permits/{permit_id}/outcome",
            json={
                **outcome_payload,
                "request_auth": self._signed_auth("permit.outcome", outcome_payload),
            },
            headers=self._auth_headers(),
        )

        return ExecuteResult(
            tx_hash=tx_hash,
            permit_id=permit_id,
            requested_amount=amount,
            approved_amount=approved_amount,
            partial=is_partial,
        )

    async def get_headroom(self, wallet: str) -> WalletHeadroom:
        self._check_state()
        await self._ensure_runtime_ticket()
        data = await self.get_status()
        for wa in data.get("wallet_access", []):
            if wa["wallet_address"] == wallet:
                max_usd = float(wa.get("agent_max_usd", "0"))
                aum = float(wa.get("current_wallet_aum_usd", "0"))
                return WalletHeadroom(
                    wallet_address=wallet,
                    max_usd=max_usd,
                    reserved_usd=0.0,
                    available_usd=max_usd,
                    wallet_aum_usd=aum,
                )
        raise WalletNotAssignedError(f"Wallet {wallet} not assigned to this agent")

    async def get_active_permits(self) -> list[dict]:
        self._check_state()
        logger.info("get_active_permits is not yet implemented on the coordinator")
        return []

    async def close(self) -> None:
        if self._connection_task:
            self._connection_task.cancel()
        if self._heartbeat_task:
            self._heartbeat_task.cancel()
        if self._ws:
            await self._ws.stop()
        if self._session:
            await self._session.close()
        self._runtime_started = False

    async def _poll_until_active(self) -> None:
        start = time.monotonic()
        while True:
            if time.monotonic() - start > self._connect_timeout:
                self._reason_code = "CONNECT_TIMEOUT"
                raise ConnectTimeoutError()

            await asyncio.sleep(3.0)
            await self.get_status()

            if self._connection_phase == "COMPLETE" and self._status == "ACTIVE":
                await self._ensure_runtime_started()
                return

            if self._connection_phase == "FAILED":
                return

    def _signed_auth(self, op_name: str, payload: dict | str) -> dict:
        if not self._agent_id:
            raise SynodConnectionError("Agent has not connected yet")
        return build_signed_request_auth(self._keypair, self._agent_id, op_name, payload)

    def _auth_headers(self) -> dict[str, str]:
        if not self._session_token:
            raise SynodConnectionError("Agent session is not established")
        return {"Authorization": f"Bearer {self._session_token}"}

    def _check_state(self) -> None:
        if self._treasury_halted:
            raise TreasuryHaltedError()
        if self._agent_suspended:
            raise AgentSuspendedError()
        if self._status != "ACTIVE" or self._connection_phase != "COMPLETE":
            raise AgentNotActiveError(self._reason_code or self._status)

    async def _ensure_session(self) -> None:
        if self._session is None or self._session.closed:
            self._session = aiohttp.ClientSession()

    def _seconds_until_ticket_expiry(self) -> float | None:
        if not self._ticket_expires_at:
            return None

        try:
            expires_at = datetime.fromisoformat(self._ticket_expires_at.replace("Z", "+00:00"))
        except ValueError:
            return None

        return (expires_at - datetime.now(timezone.utc)).total_seconds()

    async def _ensure_runtime_ticket(self, min_ttl_seconds: int = 120) -> None:
        if not self._session_token:
            return

        seconds_left = self._seconds_until_ticket_expiry()
        if seconds_left is not None and seconds_left > min_ttl_seconds:
            return

        await self._refresh_runtime_ticket()

    async def _refresh_runtime_ticket(self, websocket_only: bool = False) -> None:
        if not self._session_token:
            raise SynodConnectionError("Agent session is not established")

        await self._ensure_session()
        resp = await self._session.post(
            f"{self._coordinator_url}/v1/agents/ws-ticket/refresh",
            json={"websocket_only": websocket_only},
            headers=self._auth_headers(),
        )
        if not resp.ok:
            try:
                data = await resp.json()
            except Exception:
                data = {}
            raise SynodConnectionError(f"Failed to refresh runtime ticket: {data}")

        data = await resp.json()
        self._session_token = data.get("session_token", self._session_token)
        self._websocket_token = data.get("websocket_token", self._websocket_token)
        self._ticket_expires_at = data.get("expires_at", self._ticket_expires_at)

        if self._ws and self._agent_id:
            ws_base = self._coordinator_url.replace("http://", "ws://").replace("https://", "wss://")
            self._ws.update_url(f"{ws_base}/v1/agents/ws/{self._agent_id}?token={self._websocket_token}")

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

        await self._ensure_runtime_ticket()
        ws_base = self._coordinator_url.replace("http://", "ws://").replace("https://", "wss://")
        ws_url = f"{ws_base}/v1/agents/ws/{self._agent_id}?token={self._websocket_token}"
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
        if error_code == "CONCURRENT_LIMIT":
            raise ConcurrentLimitError()
        if error_code == "TREASURY_HALTED":
            raise TreasuryHaltedError()
        if error_code == "AGENT_SUSPENDED":
            raise AgentSuspendedError()
        if "allocation" in message.lower():
            raise AllocationLimitError(0, 0, requested)
        if "tier" in message.lower():
            raise TierLimitError(0, requested)
        if "drawdown" in message.lower():
            raise DrawdownLimitError()
        raise PermitDeniedError(message)

    async def _handle_ws_event(self, event: dict) -> None:
        event_type = event.get("type", "")
        if event_type == "WALLET_AUM_UPDATE":
            wallet = event.get("wallet_address")
            new_max = event.get("agent_new_max_usd")
            for wa in self._wallet_access:
                if wa["wallet_address"] == wallet:
                    wa["current_wallet_aum_usd"] = event.get("new_aum_usd", wa.get("current_wallet_aum_usd"))
                    wa["agent_max_usd"] = str(new_max) if new_max else wa.get("agent_max_usd")
        elif event_type == "TREASURY_HALTED":
            self._treasury_halted = True
        elif event_type == "TREASURY_RESUMED":
            self._treasury_halted = False
        elif event_type == "AGENT_SUSPENDED":
            self._agent_suspended = True
        elif event_type == "CONSTITUTION_UPDATED":
            try:
                await self.get_status()
            except Exception as exc:  # pragma: no cover - best effort
                logger.error("Failed to refresh status after constitution update: %s", exc)

    async def _handle_ws_disconnect(self) -> None:
        try:
            await self._refresh_runtime_ticket(websocket_only=True)
        except Exception as exc:  # pragma: no cover - best effort
            logger.warning("WebSocket ticket refresh failed: %s", exc)

        try:
            await self.get_status()
        except Exception:
            pass

    async def _heartbeat_loop(self) -> None:
        while True:
            await asyncio.sleep(60)
            try:
                await self._ensure_runtime_ticket()
                await self._session.post(
                    f"{self._coordinator_url}/v1/agents/{self._agent_id}/heartbeat",
                    headers=self._auth_headers(),
                )
            except Exception as exc:  # pragma: no cover - best effort
                logger.warning("Heartbeat failed: %s", exc)
