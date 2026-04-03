use axum::extract::{Path, State};
use axum::{routing::{get, post}, Json, Router};
use chrono::{DateTime, Utc, Duration};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use tracing::info;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;
use crate::constitution::{ConstitutionContent, validate_constitution, generate_state_hash};

// ── Models ──

#[derive(Debug, Serialize, Deserialize)]
pub struct Proposal {
    pub proposal_id: Uuid,
    pub treasury_id: Uuid,
    pub proposer_id: Uuid,
    pub proposed_content: ConstitutionContent,
    pub status: String, // "PENDING", "EXECUTED", "EXPIRED", "WITHDRAWN"
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub signatures: Vec<ProposalSignature>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProposalSignature {
    pub signature_id: Uuid,
    pub proposal_id: Uuid,
    pub signer_wallet: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateProposalRequest {
    pub content: ConstitutionContent,
}

#[derive(Debug, Deserialize)]
pub struct SignProposalRequest {
    pub wallet_address: String,
    pub signature_base64: String, // Ed25519 signature of the proposed state hash
}

// ── Endpoints ──

pub async fn get_treasury_proposals(
    State(state): State<AppState>,
    _auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
) -> AppResult<Json<Vec<Proposal>>> {
    let rows: Vec<(Uuid, Uuid, serde_json::Value, String, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT proposal_id, proposer_id, proposed_content, status, created_at, expires_at 
        FROM proposals 
        WHERE treasury_id = $1 
        ORDER BY created_at DESC
        "#
    )
    .bind(treasury_id)
    .fetch_all(&state.db)
    .await?;

    let mut proposals = Vec::new();
    for (pid, proposer, content_raw, status, created, expires) in rows {
        let content: ConstitutionContent = serde_json::from_value(content_raw).unwrap();
        
        let sigs: Vec<(Uuid, String, DateTime<Utc>)> = sqlx::query_as(
            "SELECT signature_id, signer_wallet, created_at FROM proposal_signatures WHERE proposal_id = $1"
        )
        .bind(pid)
        .fetch_all(&state.db)
        .await?;

        let signatures = sigs.into_iter().map(|(id, wallet, created_at)| ProposalSignature {
            signature_id: id,
            proposal_id: pid,
            signer_wallet: wallet,
            created_at,
        }).collect();

        proposals.push(Proposal {
            proposal_id: pid,
            treasury_id,
            proposer_id: proposer,
            proposed_content: content,
            status,
            created_at: created,
            expires_at: expires,
            signatures,
        });
    }

    Ok(Json(proposals))
}

pub async fn create_proposal(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(treasury_id): Path<Uuid>,
    Json(payload): Json<CreateProposalRequest>,
) -> AppResult<(StatusCode, Json<Proposal>)> {
    // 1. Validate rules
    let validation = validate_constitution(&payload.content);
    if !validation.valid {
        return Err(AppError::InvalidInput(validation.errors.join(", ")));
    }

    // 2. Check if active proposal exists
    let active_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM proposals WHERE treasury_id = $1 AND status = 'PENDING' AND expires_at > NOW()"
    )
    .bind(treasury_id)
    .fetch_one(&state.db)
    .await?;

    if active_count.0 > 0 {
        return Err(AppError::InvalidInput("An active proposal already exists for this treasury".to_string()));
    }

    // 3. Insert proposal (72h default TTL)
    let proposal_id = Uuid::new_v4();
    let content_json = serde_json::to_value(&payload.content).unwrap();
    let expires_at = Utc::now() + Duration::hours(72);

    sqlx::query(
        r#"
        INSERT INTO proposals (proposal_id, treasury_id, proposer_id, proposed_content, status, expires_at) 
        VALUES ($1, $2, $3, $4, 'PENDING', $5)
        "#
    )
    .bind(proposal_id)
    .bind(treasury_id)
    .bind(auth.user_id)
    .bind(&content_json)
    .bind(expires_at)
    .execute(&state.db)
    .await?;

    info!(treasury = %treasury_id, proposal = %proposal_id, "New constitution proposal created");

    // Re-fetch to return complete state
    let proposals_res = get_treasury_proposals(State(state), auth, Path(treasury_id)).await?.0;
    let proposal = proposals_res.into_iter().find(|p| p.proposal_id == proposal_id).unwrap();

    Ok((StatusCode::CREATED, Json(proposal)))
}

pub async fn sign_proposal(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((treasury_id, proposal_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<SignProposalRequest>,
) -> AppResult<(StatusCode, Json<Proposal>)> {
    // 1. Fetch proposal
    let proposal_row: Option<(String, DateTime<Utc>, serde_json::Value)> = sqlx::query_as(
        "SELECT status, expires_at, proposed_content FROM proposals WHERE proposal_id = $1"
    )
    .bind(proposal_id)
    .fetch_optional(&state.db)
    .await?;

    let (status, expires_at, content_json) = proposal_row.ok_or_else(|| AppError::NotFound("Proposal not found".to_string()))?;

    // 2. Check status and expiry
    if status != "PENDING" {
        return Err(AppError::InvalidInput(format!("Cannot sign proposal in {} state", status)));
    }
    if Utc::now() > expires_at {
        sqlx::query("UPDATE proposals SET status = 'EXPIRED' WHERE proposal_id = $1")
            .bind(proposal_id)
            .execute(&state.db)
            .await?;
        return Err(AppError::InvalidInput("Proposal has expired".to_string()));
    }

    // 3. Verify the signer is a registered wallet for this treasury
    let wallet_active: Option<(String,)> = sqlx::query_as(
        "SELECT status FROM treasury_wallets WHERE treasury_id = $1 AND wallet_address = $2 AND status = 'ACTIVE'"
    )
    .bind(treasury_id)
    .bind(&payload.wallet_address)
    .fetch_optional(&state.db)
    .await?;

    if wallet_active.is_none() {
        return Err(AppError::InvalidInput("Signer is not an active wallet for this treasury".into()));
    }

    // 4. Verify duplicate signature
    let sig_exists: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM proposal_signatures WHERE proposal_id = $1 AND signer_wallet = $2"
    )
    .bind(proposal_id)
    .bind(&payload.wallet_address)
    .fetch_one(&state.db)
    .await?;

    if sig_exists.0 > 0 {
        return Err(AppError::InvalidInput("Wallet has already signed this proposal".into()));
    }

    // 5. Cryptographic Verification
    let content: ConstitutionContent = serde_json::from_value(content_json.clone()).unwrap();
    let proposal_hash = generate_state_hash(&content)?;
    let msg_bytes = format!("SYNOD_PROPOSAL:{}", proposal_hash).into_bytes();
    
    // Use the verify_stellar_signature function from Phase 3
    crate::stellar::verify_stellar_signature(
        &payload.wallet_address,
        &msg_bytes,
        &payload.signature_base64,
        &state.config.stellar.network_passphrase,
    )?;

    // 6. Insert signature
    let signature_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO proposal_signatures (signature_id, proposal_id, signer_wallet) VALUES ($1, $2, $3)"
    )
    .bind(signature_id)
    .bind(proposal_id)
    .bind(&payload.wallet_address)
    .execute(&state.db)
    .await?;

    info!(proposal = %proposal_id, signer = %payload.wallet_address, "Proposal signed");

    // 7. Check if Threshold Met
    let sig_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM proposal_signatures WHERE proposal_id = $1"
    )
    .bind(proposal_id)
    .fetch_one(&state.db)
    .await?;

    let wallet_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM treasury_wallets WHERE treasury_id = $1 AND status = 'ACTIVE'"
    )
    .bind(treasury_id)
    .fetch_one(&state.db)
    .await?;

    // Majority threshold calculation (e.g., 2 of 3)
    let threshold = (wallet_count.0 / 2) + 1;

    if sig_count.0 >= threshold {
        // Execute the proposal!
        info!(proposal = %proposal_id, threshold = threshold, "Threshold met, executing proposal");

        // Set status to EXECUTED
        sqlx::query("UPDATE proposals SET status = 'EXECUTED' WHERE proposal_id = $1")
            .bind(proposal_id)
            .execute(&state.db)
            .await?;

        // Determine next version
        let max_v: i32 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM constitution_history WHERE treasury_id = $1")
            .bind(treasury_id)
            .fetch_one(&state.db)
            .await?;
        let next_version = max_v + 1;

        // Apply to history
        sqlx::query(
            "INSERT INTO constitution_history (treasury_id, version, state_hash, content) VALUES ($1, $2, $3, $4)"
        )
        .bind(treasury_id)
        .bind(next_version)
        .bind(&proposal_hash)
        .bind(&content_json)
        .execute(&state.db)
        .await?;

        // Update Treasury Version
        sqlx::query("UPDATE treasuries SET constitution_version = $1, updated_at = $2 WHERE treasury_id = $3")
            .bind(next_version)
            .bind(Utc::now())
            .bind(treasury_id)
            .execute(&state.db)
            .await?;

        // Cache new constitution in Redis
        use redis::AsyncCommands;
        let mut redis = state.redis.clone();
        let cache_key = format!("constitution:{}", treasury_id);
        let _: () = redis.set(&cache_key, serde_json::to_string(&content).unwrap()).await.unwrap_or(());

        let _ = state.tx_events.send(crate::TreasuryEvent::ConstitutionUpdate {
            treasury_id,
            version: next_version,
        });
    }

    // Re-fetch to return updated proposal
    let proposals_res = get_treasury_proposals(State(state), auth, Path(treasury_id)).await?.0;
    let updated_proposal = proposals_res.into_iter().find(|p| p.proposal_id == proposal_id).unwrap();

    Ok((StatusCode::OK, Json(updated_proposal)))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:treasury_id/proposals", get(get_treasury_proposals).post(create_proposal))
        .route("/:treasury_id/proposals/:proposal_id/sign", post(sign_proposal))
}
