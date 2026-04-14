# Synod MCP Skill

## Purpose
- Run `synod-mcp` as a stdio MCP server for agent runtimes like Codex CLI or Claude Code.
- Use one local Stellar keypair as the Synod agent identity, runtime signer, and websocket reconnect identity.

## Install
- `pip install -e synod-sdk-python[dev]`
- Or package and install `synod-sdk` normally, then run `synod-mcp`

## Runtime Environment
- `SYNOD_COORDINATOR_URL` — Synod coordinator base URL, default `http://localhost:8080`
- `SYNOD_KEY_STORAGE_PATH` — local directory for the generated agent keypair, default `./synod_keys`
- `SYNOD_AGENT_SECRET` — optional existing Stellar secret; overrides generated storage
- `SYNOD_NETWORK` — `testnet` or `mainnet`
- `SYNOD_CONNECT_TIMEOUT` — seconds to wait while the slot is still pending
- `SYNOD_REJECT_PARTIAL` — `true` to reject partial permit approvals

## Enrollment Flow
1. Start `synod-mcp` from the host agent.
2. Call `synod_public_key`.
3. Copy the returned public key into the Synod dashboard slot.
4. Wallet-sign the binding from the dashboard.
5. Call `synod_connect`.
6. After activation, Synod returns the runtime session and websocket ticket automatically.

## Available Tools
- `synod_public_key` — returns the local public key and enrollment instructions
- `synod_connect` — completes the Synod Connect challenge flow
- `synod_status` — returns slot status, connection phase, and wallet access
- `synod_wallet_headroom` — returns current headroom for an assigned wallet
- `synod_execute_payment` — requests a permit, signs the Stellar payment, and reports outcome

## Operational Notes
- Session tokens are short-lived and refreshed automatically before expiry.
- WebSocket tickets are refreshed on reconnect without rotating the local keypair.
- If the public key changes, revoke the old slot and bind the new key explicitly.
- Capital allocation, wallet access, and tier limits remain governed by Synod policy, not the MCP server.
