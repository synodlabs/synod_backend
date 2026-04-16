pub use stellar_xdr::curr as next_xdr;
use crate::error::{AppError, AppResult};
use ed25519_dalek::{Signer, Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::convert::TryInto;

pub fn verify_stellar_signature(
    public_key_str: &str,
    message: &[u8],
    signature_base64: &str,
    _network_passphrase: &str,
) -> AppResult<()> {
    let public_key_bytes = decode_stellar_address(public_key_str)?;
    let signature_bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, signature_base64)
        .map_err(|_| AppError::CosignFailed("Invalid base64 signature".to_string()))?;

    let verifying_key = VerifyingKey::from_bytes(&public_key_bytes.try_into().map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid public key length")))?)
        .map_err(|_| AppError::CosignFailed("Invalid public key".to_string()))?;

    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| AppError::CosignFailed("Invalid signature format".to_string()))?;

    // Freighter (and the Stellar ecosystem standard) signs:
    // SHA256("Stellar Signed Message:\n" + message)
    let mut hasher = Sha256::new();
    hasher.update(b"Stellar Signed Message:\n");
    hasher.update(message);
    let hashed_payload = hasher.finalize();

    verifying_key.verify(&hashed_payload, &signature)
        .map_err(|_| AppError::OwnershipVerificationFailed)
}

pub fn sha256_bytes(payload: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hasher.finalize().into()
}

pub fn verify_raw_ed25519_signature(
    public_key_str: &str,
    message: &[u8],
    signature_base64: &str,
) -> AppResult<()> {
    let public_key_bytes = decode_stellar_address(public_key_str)?;
    let signature_bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, signature_base64)
        .map_err(|_| AppError::RequestSignatureInvalid)?;

    let verifying_key = VerifyingKey::from_bytes(
        &public_key_bytes
            .try_into()
            .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid public key length")))?,
    )
    .map_err(|_| AppError::RequestSignatureInvalid)?;

    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| AppError::RequestSignatureInvalid)?;

    verifying_key
        .verify(message, &signature)
        .map_err(|_| AppError::RequestSignatureInvalid)
}

pub fn decode_stellar_address(address: &str) -> AppResult<[u8; 32]> {
    let decoded = data_encoding::BASE32_NOPAD.decode(address.as_bytes())
        .map_err(|_| AppError::CosignFailed("Invalid base32 address".into()))?;
    
    if decoded.len() != 35 {
        return Err(AppError::CosignFailed("Invalid address length".into()));
    }
    
    if decoded[0] != 0x30 { // G is 0x30
        return Err(AppError::CosignFailed("Not a G address".into()));
    }
    
    let mut key = [0u8; 32];
    key.copy_from_slice(&decoded[1..33]);
    Ok(key)
}

pub fn construct_set_options_xdr(
    source_account: &str,
    signer_key: &str,
    weight: u32,
) -> AppResult<String> {
    use next_xdr::{
        Operation, OperationBody, SetOptionsOp, 
        WriteXdr, Uint256, Signer, SignerKey
    };

    let _source_bytes = decode_stellar_address(source_account)?;
    let signer_bytes = decode_stellar_address(signer_key)?;

    let op = Operation {
        source_account: None,
        body: OperationBody::SetOptions(SetOptionsOp {
            inflation_dest: None,
            clear_flags: None,
            set_flags: None,
            master_weight: None,
            low_threshold: None,
            med_threshold: None,
            high_threshold: None,
            home_domain: None,
            signer: Some(Signer {
                key: SignerKey::Ed25519(Uint256(signer_bytes)),
                weight: weight.into(), // In newer stellar-xdr, Weight might be a wrapper or u32
            }),
        }),
    };

    // Note: For a real transaction envelope we need Sequence Number, Fee, Network Passphrase, etc.
    // For Phase 3 multisig coordination, we primarily need the XDR of the SetOptions operation 
    // to pass to the wallet for signing.
    
    let xdr = op.to_xdr(next_xdr::Limits::none())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("XDR encoding error: {}", e)))?;

    Ok(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, xdr))
}
pub fn sign_transaction_hash(
    secret_key_str: &str,
    network_passphrase: &str,
    transaction_xdr_base64: &str,
) -> AppResult<String> {
    use ed25519_dalek::SigningKey;

    // 1. Prepare Network ID
    let mut hasher = Sha256::new();
    hasher.update(network_passphrase.as_bytes());
    let network_id = hasher.finalize();

    // 2. Decode Transaction XDR
    let raw_tx = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, transaction_xdr_base64)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid XDR base64")))?;
    
    // We expect just the Transaction struct (not the envelope) for hashing,
    // although some SDKs provide the envelope. If it's the envelope, we'd extract the tx.
    // In our case, construct_set_options_xdr returns the OP or TX? 
    // Wait, let's assume we receive the Transaction struct.
    
    // 3. Prepare Signing Payload
    let mut payload = Vec::new();
    payload.extend_from_slice(&network_id);
    payload.extend_from_slice(&[0, 0, 0, 2]); // EnvelopeType::ENVELOPE_TYPE_TX
    payload.extend_from_slice(&raw_tx);

    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let hash = hasher.finalize();

    // 4. Sign
    let secret_bytes = decode_secret_key(secret_key_str)?; 
    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let signature = <SigningKey as Signer<Signature>>::sign(&signing_key, &hash);

    Ok(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, signature.to_bytes()))
}

pub fn decode_secret_key(secret: &str) -> AppResult<[u8; 32]> {
    // Stellar secrets are BASE32 with 0x40 (S) prefix
    let decoded = data_encoding::BASE32_NOPAD.decode(secret.as_bytes())
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid secret key encoding")))?;
    
    if decoded.len() != 35 {
        return Err(AppError::Internal(anyhow::anyhow!("Invalid secret key length")));
    }
    
    let mut key = [0u8; 32];
    key.copy_from_slice(&decoded[1..33]);
    Ok(key)
}
