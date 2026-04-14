"""Minimal stdio MCP server for Synod Connect."""

from __future__ import annotations

import asyncio
import json
import os
import sys
from typing import Any

from . import SynodAgent

PROTOCOL_VERSION = "2024-11-05"


def _tool_response(text: str, structured: dict[str, Any] | None = None) -> dict[str, Any]:
    payload: dict[str, Any] = {
        "content": [{"type": "text", "text": text}],
        "isError": False,
    }
    if structured is not None:
        payload["structuredContent"] = structured
    return payload


def _tool_error(message: str) -> dict[str, Any]:
    return {
        "content": [{"type": "text", "text": message}],
        "isError": True,
    }


class SynodMCPServer:
    def __init__(self) -> None:
        coordinator_url = os.getenv("SYNOD_COORDINATOR_URL", "http://localhost:8080")
        key_storage_path = os.getenv("SYNOD_KEY_STORAGE_PATH", "./synod_keys")
        existing_secret_key = os.getenv("SYNOD_AGENT_SECRET")
        network = os.getenv("SYNOD_NETWORK", "testnet")
        connect_timeout = int(os.getenv("SYNOD_CONNECT_TIMEOUT", "900"))
        reject_partial = os.getenv("SYNOD_REJECT_PARTIAL", "").lower() in {"1", "true", "yes"}
        log_level = os.getenv("SYNOD_LOG_LEVEL", "INFO")

        self.agent = SynodAgent(
            key_storage_path=None if existing_secret_key else key_storage_path,
            existing_secret_key=existing_secret_key,
            coordinator_url=coordinator_url,
            network=network,
            reject_partial=reject_partial,
            connect_timeout_seconds=connect_timeout,
            log_level=log_level,
        )

    def _tools(self) -> list[dict[str, Any]]:
        return [
            {
                "name": "synod_public_key",
                "description": "Return the local Synod agent public key and enrollment instructions.",
                "inputSchema": {"type": "object", "properties": {}},
            },
            {
                "name": "synod_connect",
                "description": "Complete the Synod Connect challenge flow after the public key has been bound in the dashboard.",
                "inputSchema": {"type": "object", "properties": {}},
            },
            {
                "name": "synod_status",
                "description": "Return the current Synod runtime status for this agent.",
                "inputSchema": {"type": "object", "properties": {}},
            },
            {
                "name": "synod_wallet_headroom",
                "description": "Return the currently available capital headroom for one assigned wallet.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"wallet": {"type": "string"}},
                    "required": ["wallet"],
                },
            },
            {
                "name": "synod_execute_payment",
                "description": "Request a permit, sign the Stellar payment, and submit it to Synod for co-signing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "wallet": {"type": "string"},
                        "destination": {"type": "string"},
                        "amount": {"type": "number"},
                        "asset": {"type": "string"},
                        "asset_issuer": {"type": "string"},
                        "reject_partial": {"type": "boolean"},
                    },
                    "required": ["wallet", "destination", "amount", "asset"],
                },
            },
        ]

    async def handle(self, message: dict[str, Any]) -> dict[str, Any] | None:
        method = message.get("method")

        if method == "notifications/initialized":
            return None

        if method == "ping":
            return {"jsonrpc": "2.0", "id": message.get("id"), "result": {}}

        if method == "initialize":
            return {
                "jsonrpc": "2.0",
                "id": message.get("id"),
                "result": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "serverInfo": {"name": "synod-mcp", "version": "0.1.0"},
                    "capabilities": {"tools": {}},
                },
            }

        if method == "tools/list":
            return {
                "jsonrpc": "2.0",
                "id": message.get("id"),
                "result": {"tools": self._tools()},
            }

        if method != "tools/call":
            return self._error(message.get("id"), -32601, f"Unknown method: {method}")

        params = message.get("params", {})
        tool_name = params.get("name")
        arguments = params.get("arguments", {}) or {}

        try:
            if tool_name == "synod_public_key":
                result = _tool_response(
                    "Paste this public key into Synod, bind it to an agent slot, then call synod_connect.",
                    {"public_key": self.agent.public_key},
                )
            elif tool_name == "synod_connect":
                data = await self.agent.connect()
                result = _tool_response(
                    f"Connected agent {data['agent_id']} with status {data['slot_status']} ({data['connection_phase']}).",
                    data,
                )
            elif tool_name == "synod_status":
                data = await self.agent.get_status()
                result = _tool_response(
                    f"Agent status is {data.get('slot_status', 'UNKNOWN')} ({data.get('connection_phase', 'UNKNOWN')}).",
                    data,
                )
            elif tool_name == "synod_wallet_headroom":
                headroom = await self.agent.get_headroom(arguments["wallet"])
                data = {
                    "wallet_address": headroom.wallet_address,
                    "max_usd": headroom.max_usd,
                    "reserved_usd": headroom.reserved_usd,
                    "available_usd": headroom.available_usd,
                    "wallet_aum_usd": headroom.wallet_aum_usd,
                }
                result = _tool_response(
                    f"Wallet {headroom.wallet_address} has ${headroom.available_usd:.2f} available headroom.",
                    data,
                )
            elif tool_name == "synod_execute_payment":
                execution = await self.agent.execute(
                    wallet=arguments["wallet"],
                    destination=arguments["destination"],
                    amount=float(arguments["amount"]),
                    asset=arguments["asset"],
                    asset_issuer=arguments.get("asset_issuer"),
                    reject_partial=arguments.get("reject_partial"),
                )
                data = {
                    "tx_hash": execution.tx_hash,
                    "permit_id": execution.permit_id,
                    "requested_amount": execution.requested_amount,
                    "approved_amount": execution.approved_amount,
                    "partial": execution.partial,
                }
                result = _tool_response(
                    f"Executed payment via permit {execution.permit_id} with tx hash {execution.tx_hash}.",
                    data,
                )
            else:
                result = _tool_error(f"Unknown tool: {tool_name}")

            return {"jsonrpc": "2.0", "id": message.get("id"), "result": result}
        except Exception as exc:
            return {
                "jsonrpc": "2.0",
                "id": message.get("id"),
                "result": _tool_error(str(exc)),
            }

    def _error(self, request_id: Any, code: int, message: str) -> dict[str, Any]:
        return {
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": code, "message": message},
        }


async def _read_message() -> dict[str, Any] | None:
    headers: dict[str, str] = {}
    while True:
        line = await asyncio.to_thread(sys.stdin.buffer.readline)
        if not line:
            return None

        decoded = line.decode("utf-8")
        if decoded in {"\r\n", "\n", ""}:
            break

        name, value = decoded.split(":", 1)
        headers[name.strip().lower()] = value.strip()

    content_length = int(headers.get("content-length", "0"))
    if content_length <= 0:
        return None

    body = await asyncio.to_thread(sys.stdin.buffer.read, content_length)
    if not body:
        return None
    return json.loads(body.decode("utf-8"))


async def _write_message(message: dict[str, Any]) -> None:
    encoded = json.dumps(message).encode("utf-8")
    header = f"Content-Length: {len(encoded)}\r\n\r\n".encode("utf-8")
    await asyncio.to_thread(sys.stdout.buffer.write, header + encoded)
    await asyncio.to_thread(sys.stdout.buffer.flush)


async def main() -> None:
    server = SynodMCPServer()
    try:
        while True:
            message = await _read_message()
            if message is None:
                break
            response = await server.handle(message)
            if response is not None:
                await _write_message(response)
    finally:
        await server.agent.close()


def cli() -> None:
    asyncio.run(main())


if __name__ == "__main__":
    cli()
