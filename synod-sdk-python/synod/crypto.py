"""Synod Agent SDK — keypair management and cryptographic operations.

SECURITY: The agent's private key is encrypted at rest using AES-256-GCM
with a key derived from the API key via PBKDF2. The plaintext private key
is NEVER logged or exposed outside of signing operations.
"""

import os
import json
import hashlib
import logging
from pathlib import Path

from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
from cryptography.hazmat.primitives import hashes
from stellar_sdk import Keypair

logger = logging.getLogger("synod.crypto")


def _derive_encryption_key(api_key: str, salt: bytes) -> bytes:
    """Derive a 256-bit AES key from the API key using PBKDF2."""
    kdf = PBKDF2HMAC(
        algorithm=hashes.SHA256(),
        length=32,
        salt=salt,
        iterations=100_000,
    )
    return kdf.derive(api_key.encode("utf-8"))


def generate_keypair(api_key: str, storage_path: str) -> Keypair:
    """Generate a new Stellar Ed25519 keypair and store encrypted at rest.

    If a keypair file already exists at the storage path, loads and decrypts it.
    """
    key_file = Path(storage_path) / "synod_agent_key.enc"
    meta_file = Path(storage_path) / "synod_agent_key.meta"

    if key_file.exists() and meta_file.exists():
        return load_keypair(api_key, storage_path)

    # Generate fresh keypair
    kp = Keypair.random()
    logger.info("Generated new agent keypair: %s", kp.public_key)

    # Encrypt and store
    _store_keypair(kp, api_key, storage_path)
    return kp


def load_keypair(api_key: str, storage_path: str) -> Keypair:
    """Load and decrypt an existing keypair from storage."""
    key_file = Path(storage_path) / "synod_agent_key.enc"
    meta_file = Path(storage_path) / "synod_agent_key.meta"

    with open(meta_file, "r") as f:
        meta = json.load(f)

    salt = bytes.fromhex(meta["salt"])
    nonce = bytes.fromhex(meta["nonce"])

    encryption_key = _derive_encryption_key(api_key, salt)
    aesgcm = AESGCM(encryption_key)

    with open(key_file, "rb") as f:
        ciphertext = f.read()

    secret_key = aesgcm.decrypt(nonce, ciphertext, None).decode("utf-8")
    kp = Keypair.from_secret(secret_key)
    logger.info("Loaded existing agent keypair: %s", kp.public_key)
    return kp


def _store_keypair(kp: Keypair, api_key: str, storage_path: str) -> None:
    """Encrypt and persist a keypair to disk."""
    os.makedirs(storage_path, exist_ok=True)

    salt = os.urandom(16)
    nonce = os.urandom(12)

    encryption_key = _derive_encryption_key(api_key, salt)
    aesgcm = AESGCM(encryption_key)

    ciphertext = aesgcm.encrypt(nonce, kp.secret.encode("utf-8"), None)

    key_file = Path(storage_path) / "synod_agent_key.enc"
    meta_file = Path(storage_path) / "synod_agent_key.meta"

    with open(key_file, "wb") as f:
        f.write(ciphertext)

    with open(meta_file, "w") as f:
        json.dump({
            "salt": salt.hex(),
            "nonce": nonce.hex(),
            "public_key": kp.public_key,
        }, f)


def keypair_from_secret(secret_key: str) -> Keypair:
    """Wrap an existing secret key into a Stellar Keypair.

    Used for Path 2 initialization where the agent already controls the wallet's key.
    """
    return Keypair.from_secret(secret_key)
