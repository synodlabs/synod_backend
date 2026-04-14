"""Synod Agent SDK cryptography helpers for Synod Connect."""

from __future__ import annotations

import base64
import json
import os
import time
import uuid
from pathlib import Path
from typing import Any

from stellar_sdk import Keypair


KEY_FILE = "synod_agent_key.json"


def generate_keypair(storage_path: str) -> Keypair:
    """Generate a new keypair or load an existing one from local storage."""
    key_file = Path(storage_path) / KEY_FILE
    if key_file.exists():
        return load_keypair(storage_path)

    keypair = Keypair.random()
    _store_keypair(keypair, storage_path)
    return keypair


def load_keypair(storage_path: str) -> Keypair:
    key_file = Path(storage_path) / KEY_FILE
    with open(key_file, "r", encoding="utf-8") as handle:
        data = json.load(handle)
    return Keypair.from_secret(data["secret_key"])


def _store_keypair(keypair: Keypair, storage_path: str) -> None:
    os.makedirs(storage_path, exist_ok=True)
    key_file = Path(storage_path) / KEY_FILE
    payload = {
        "public_key": keypair.public_key,
        "secret_key": keypair.secret,
        "created_at": int(time.time()),
    }
    with open(key_file, "w", encoding="utf-8") as handle:
        json.dump(payload, handle)


def keypair_from_secret(secret_key: str) -> Keypair:
    return Keypair.from_secret(secret_key)


def sign_stellar_message(keypair: Keypair, message: str) -> str:
    signature = keypair.sign_message(message)
    return base64.b64encode(signature).decode("ascii")


def canonical_json(payload: Any) -> str:
    return json.dumps(payload, separators=(",", ":"), sort_keys=False)


def build_signed_request_auth(
    keypair: Keypair,
    agent_id: str,
    op_name: str,
    payload: Any,
) -> dict[str, Any]:
    request_id = str(uuid.uuid4())
    timestamp = int(time.time())
    payload_json = canonical_json(payload)
    message = f"synod-request:{op_name}:{agent_id}:{request_id}:{timestamp}:{payload_json}"
    return {
        "agent_pubkey": keypair.public_key,
        "request_id": request_id,
        "timestamp": timestamp,
        "signature": sign_stellar_message(keypair, message),
    }
