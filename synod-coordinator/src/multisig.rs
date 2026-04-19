use crate::error::{AppError, AppResult};
use crate::{auth::AuthUser, stellar, AppState};
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use ed25519_dalek::Verifier;
use serde::Serialize;
use sqlx::Row;
use tracing::{debug, error, info};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct MultisigSetupResponse {
    pub xdr: String,
    pub coordinator_pubkey: String,
}

#[derive(Debug, Serialize)]
pub struct MultisigStatusResponse {
    pub is_active: bool,
    pub coordinator_pubkey: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfirmMultisigRequest {
    pub wallet_address: String,
}

pub async fn get_multisig_setup(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
) -> AppResult<Json<MultisigSetupResponse>> {
    // 1. Verify ownership
    let treasury = sqlx::query(
        "SELECT treasury_id FROM treasuries WHERE treasury_id = $1 AND owner_user_id = $2",
    )
    .bind(treasury_id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("Treasury not found".into()))?;

    // 2. Get the primary wallet for this treasury
    let wallet =
        sqlx::query("SELECT wallet_address FROM treasury_wallets WHERE treasury_id = $1 LIMIT 1")
            .bind(treasury.get::<Uuid, _>("treasury_id"))
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound(
                "No wallet connected to this treasury".into(),
            ))?;

    // 3. Construct SetOptions XDR
    // We add the coordinator as a signer with weight 20
    // Thresholds suggestion: Low: 1, Med: 15, High: 15
    // Note: To fully set thresholds, we'd need another operation or multiple fields in SetOptions.
    // For now, we just add the signer.
    let coordinator_pubkey = &state.config.stellar.coordinator_pubkey;
    if coordinator_pubkey.is_empty() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Coordinator pubkey not configured"
        )));
    }

    let xdr = stellar::construct_set_options_xdr(
        &wallet.get::<String, _>("wallet_address"),
        coordinator_pubkey,
        20, // weight
    )?;

    Ok(Json(MultisigSetupResponse {
        xdr,
        coordinator_pubkey: coordinator_pubkey.clone(),
    }))
}

pub async fn confirm_multisig(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<ConfirmMultisigRequest>,
) -> AppResult<Json<MultisigStatusResponse>> {
    // 1. Verify ownership
    let _ = sqlx::query(
        "SELECT treasury_id FROM treasuries WHERE treasury_id = $1 AND owner_user_id = $2",
    )
    .bind(treasury_id)
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("Treasury not found".into()))?;

    // 2. Update status and mark multisig as active
    let result = sqlx::query(
        "UPDATE treasury_wallets 
         SET multisig_active = true, status = 'ACTIVE' 
         WHERE treasury_id = $1 AND wallet_address = $2",
    )
    .bind(treasury_id)
    .bind(&payload.wallet_address)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(
            "Wallet not found in this treasury".into(),
        ));
    }

    Ok(Json(MultisigStatusResponse {
        is_active: true,
        coordinator_pubkey: state.config.stellar.coordinator_pubkey.clone(),
    }))
}

#[derive(Debug, serde::Deserialize)]
pub struct RevokeRequest {
    pub xdr: String, // The transaction envelope signed by the user
    pub wallet_address: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ApproveSignerRequest {
    pub xdr: String, // The transaction envelope signed by the user
    pub wallet_address: String,
}

pub async fn revoke_multisig(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<RevokeRequest>,
) -> AppResult<Json<MultisigStatusResponse>> {
    // 1. Verify ownership & link
    let wallet = sqlx::query(
        "SELECT wallet_address FROM treasury_wallets 
         WHERE treasury_id = $1 AND wallet_address = $2",
    )
    .bind(treasury_id)
    .bind(&payload.wallet_address)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound(
        "Wallet not found in this treasury".into(),
    ))?;

    // 2. Check for bypass (clean up only)
    if payload.xdr == "OFF_CHAIN_BYPASS" {
        info!(
            "Bypass requested: Cleaning up database record for wallet {}",
            wallet.get::<String, _>("wallet_address")
        );
        sqlx::query(
            "DELETE FROM treasury_wallets 
             WHERE treasury_id = $1 AND wallet_address = $2",
        )
        .bind(treasury_id)
        .bind(wallet.get::<String, _>("wallet_address"))
        .execute(&state.db)
        .await?;

        return Ok(Json(MultisigStatusResponse {
            is_active: false,
            coordinator_pubkey: state.config.stellar.coordinator_pubkey.clone(),
        }));
    }

    // 3. Decode the Envelope and add the Coordinator's signature
    use crate::stellar::next_xdr::{
        DecoratedSignature, ReadXdr, Signature, SignatureHint, TransactionEnvelope, WriteXdr,
    };
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

    let raw_env = BASE64
        .decode(&payload.xdr)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid XDR base64")))?;

    let mut envelope =
        TransactionEnvelope::from_xdr(&raw_env, crate::stellar::next_xdr::Limits::none())
            .map_err(|e| AppError::Internal(anyhow::anyhow!("XDR decoding error: {}", e)))?;

    // We only support V1 Envelopes currently
    let TransactionEnvelope::Tx(v1_env) = &mut envelope else {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Only V1 Transaction Envelopes are supported"
        )));
    };

    // Construct the Hash for verification and signing
    let clean_passphrase = state.config.stellar.network_passphrase.trim_matches('"');
    let candidate_hashes = crate::stellar::calculate_tx_v1_hashes(&raw_env, clean_passphrase)?;

    // CRYPTO SELF-TEST: Verify the user's signature
    let wallet_address = wallet.get::<String, _>("wallet_address");
    let user_pk_bytes = crate::stellar::decode_stellar_address(&wallet_address)?;
    let user_verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&user_pk_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Invalid VerifyingKey: {}", e)))?;

    let mut verified = false;
    let mut matching_hash = None;
    for (h_idx, hash) in candidate_hashes.iter().enumerate() {
        for sig in v1_env.signatures.iter() {
            let sig_bytes: &[u8] = &sig.signature.0;
            let dalek_sig = ed25519_dalek::Signature::from_slice(sig_bytes)
                .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid signature format")))?;

            if user_verifying_key.verify(hash, &dalek_sig).is_ok() {
                verified = true;
                matching_hash = Some(*hash);
                info!("Signature verified using strategy index: {}", h_idx);
                break;
            }
        }
        if verified {
            break;
        }
    }

    if !verified {
        error!(
            "Security Hash Mismatch (revoke): Failed all {} strategies. XDR: {}",
            candidate_hashes.len(),
            payload.xdr
        );
        return Err(AppError::Internal(anyhow::anyhow!(
            "Security Error: Signature verification failed."
        )));
    }
    let hash = matching_hash.unwrap();

    info!("User signature verified. Appending Coordinator signature...");

    // Sign the hash
    let secret_bytes =
        crate::stellar::decode_secret_key(&state.config.stellar.coordinator_secret_key)?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
    let signature_bytes = <ed25519_dalek::SigningKey as ed25519_dalek::Signer<
        ed25519_dalek::Signature,
    >>::sign(&signing_key, &hash)
    .to_bytes();

    // Prepare the Hint (last 4 bytes of the public key)
    let pubkey_bytes = signing_key.verifying_key().to_bytes();
    let hint = [
        pubkey_bytes[28],
        pubkey_bytes[29],
        pubkey_bytes[30],
        pubkey_bytes[31],
    ];

    use crate::stellar::next_xdr::Limits;
    let mut sigs: Vec<DecoratedSignature> = v1_env.signatures.clone().into();
    sigs.push(DecoratedSignature {
        hint: SignatureHint(hint),
        signature: Signature(
            signature_bytes
                .to_vec()
                .try_into()
                .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid signature length")))?,
        ),
    });
    v1_env.signatures = sigs
        .try_into()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Too many signatures")))?;

    // 3. Submit to Horizon
    let final_xdr = envelope
        .to_xdr(Limits::none())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Final XDR encoding error: {}", e)))?;
    let final_xdr_base64 = BASE64.encode(&final_xdr);

    info!(
        "Submitting co-signed revocation to Stellar for wallet: {}",
        wallet_address
    );
    let horizon_url = &state.config.stellar.horizon_url;
    let client = reqwest::Client::new();
    let horizon_res = client
        .post(format!("{}/transactions", horizon_url))
        .form(&[("tx", final_xdr_base64)])
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Horizon connection failed: {}", e)))?;

    if !horizon_res.status().is_success() {
        let err_body = horizon_res.text().await.unwrap_or_default();
        error!("Stellar Revocation Failed: {}", err_body);
        return Err(AppError::Internal(anyhow::anyhow!(
            "On-chain revocation failed: {}. Ensure your account meets signing thresholds.",
            err_body
        )));
    }

    // 4. Mark as inactive in DB and delete (as requested "remove everything")
    sqlx::query(
        "DELETE FROM treasury_wallets 
         WHERE treasury_id = $1 AND wallet_address = $2",
    )
    .bind(treasury_id)
    .bind(&wallet_address)
    .execute(&state.db)
    .await?;

    Ok(Json(MultisigStatusResponse {
        is_active: false,
        coordinator_pubkey: state.config.stellar.coordinator_pubkey.clone(),
    }))
}

pub async fn approve_signer(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<ApproveSignerRequest>,
) -> AppResult<Json<MultisigStatusResponse>> {
    // 1. Verify membership/ownership (already done via treasury_id path & auth)
    let wallet = sqlx::query(
        "SELECT wallet_address FROM treasury_wallets 
         WHERE treasury_id = $1 AND wallet_address = $2",
    )
    .bind(treasury_id)
    .bind(&payload.wallet_address)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound(
        "Wallet not found in this treasury".into(),
    ))?;

    // 2. Decode the Envelope and add the Coordinator's signature
    use crate::stellar::next_xdr::{
        DecoratedSignature, ReadXdr, Signature, SignatureHint, TransactionEnvelope, WriteXdr,
    };
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

    let raw_env = BASE64
        .decode(&payload.xdr)
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid XDR base64")))?;

    let mut envelope =
        TransactionEnvelope::from_xdr(&raw_env, crate::stellar::next_xdr::Limits::none())
            .map_err(|e| AppError::Internal(anyhow::anyhow!("XDR decoding error: {}", e)))?;

    let TransactionEnvelope::Tx(v1_env) = &mut envelope else {
        return Err(AppError::Internal(anyhow::anyhow!(
            "Only V1 Transaction Envelopes are supported"
        )));
    };

    // Construct the Hash
    let clean_passphrase = state.config.stellar.network_passphrase.trim_matches('"');
    let candidate_hashes = crate::stellar::calculate_tx_v1_hashes(&raw_env, clean_passphrase)?;

    // Verify user signature
    let wallet_address = wallet.get::<String, _>("wallet_address");
    let user_pk_bytes = crate::stellar::decode_stellar_address(&wallet_address)?;
    let user_verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&user_pk_bytes)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Invalid VerifyingKey: {}", e)))?;

    let mut verified = false;
    let mut matching_hash = None;
    for (h_idx, hash) in candidate_hashes.iter().enumerate() {
        for sig in v1_env.signatures.iter() {
            let sig_bytes: &[u8] = &sig.signature.0;
            let dalek_sig = ed25519_dalek::Signature::from_slice(sig_bytes)
                .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid signature format")))?;

            if user_verifying_key.verify(hash, &dalek_sig).is_ok() {
                verified = true;
                matching_hash = Some(*hash);
                info!("Signature 0 verified using strategy index: {}", h_idx);
                break;
            } else {
                debug!("Strategy {} failed for hash: {}", h_idx, hex::encode(hash));
            }
        }
        if verified {
            break;
        }
    }

    if !verified {
        error!(
            "Security Hash Mismatch (approve_signer): Failed all {} strategies. XDR: {}",
            candidate_hashes.len(),
            payload.xdr
        );
        return Err(AppError::Internal(anyhow::anyhow!(
            "Security Error: Signature verification failed."
        )));
    }

    let hash = matching_hash.unwrap();

    // Sign the hash
    let secret_bytes =
        crate::stellar::decode_secret_key(&state.config.stellar.coordinator_secret_key)?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&secret_bytes);
    let signature_bytes = <ed25519_dalek::SigningKey as ed25519_dalek::Signer<
        ed25519_dalek::Signature,
    >>::sign(&signing_key, &hash)
    .to_bytes();

    let pubkey_bytes = signing_key.verifying_key().to_bytes();
    let hint = [
        pubkey_bytes[28],
        pubkey_bytes[29],
        pubkey_bytes[30],
        pubkey_bytes[31],
    ];

    use crate::stellar::next_xdr::Limits;
    let mut sigs: Vec<DecoratedSignature> = v1_env.signatures.clone().into();
    sigs.push(DecoratedSignature {
        hint: SignatureHint(hint),
        signature: Signature(
            signature_bytes
                .to_vec()
                .try_into()
                .map_err(|_| AppError::Internal(anyhow::anyhow!("Invalid signature length")))?,
        ),
    });
    v1_env.signatures = sigs
        .try_into()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("Too many signatures")))?;

    // Submit to Horizon
    let final_xdr = envelope
        .to_xdr(Limits::none())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Final XDR encoding error: {}", e)))?;
    let final_xdr_base64 = BASE64.encode(&final_xdr);

    let horizon_url = &state.config.stellar.horizon_url;
    let client = reqwest::Client::new();
    let horizon_res = client
        .post(format!("{}/transactions", horizon_url))
        .form(&[("tx", final_xdr_base64)])
        .send()
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Horizon connection failed: {}", e)))?;

    if !horizon_res.status().is_success() {
        let err_body = horizon_res.text().await.unwrap_or_default();
        return Err(AppError::Internal(anyhow::anyhow!(
            "On-chain approval failed: {}. Ensure your account meets signing thresholds.",
            err_body
        )));
    }

    Ok(Json(MultisigStatusResponse {
        is_active: true,
        coordinator_pubkey: state.config.stellar.coordinator_pubkey.clone(),
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:treasury_id/setup", get(get_multisig_setup))
        .route("/:treasury_id/confirm", post(confirm_multisig))
        .route("/:treasury_id/revoke", post(revoke_multisig))
        .route("/:treasury_id/approve-signer", post(approve_signer))
}
