use axum::http::StatusCode;
use sqlx::Row;
use uuid::Uuid;

use crate::common::{create_treasury, generate_test_stellar_keypair, setup_test_context};

mod common;

#[serial_test::serial]
#[tokio::test]
async fn confirm_multisig_marks_the_target_wallet_active_even_if_already_active() {
    let ctx = setup_test_context().await;
    let treasury_id = create_treasury(&ctx, "Multisig Confirm Test").await;
    let (_wallet_a_key, wallet_a) = generate_test_stellar_keypair();
    let (_wallet_b_key, wallet_b) = generate_test_stellar_keypair();

    sqlx::query(
        "INSERT INTO treasury_wallets (wallet_id, treasury_id, wallet_address, label, multisig_active, status, added_at)
         VALUES
         ($1, $2, $3, 'Wallet A', false, 'ACTIVE', NOW()),
         ($4, $2, $5, 'Wallet B', false, 'PENDING', NOW())",
    )
    .bind(Uuid::new_v4())
    .bind(treasury_id)
    .bind(&wallet_a)
    .bind(Uuid::new_v4())
    .bind(&wallet_b)
    .execute(&ctx.db)
    .await
    .unwrap();

    let response = ctx
        .client
        .post(format!(
            "{}/v1/multisig/{}/confirm",
            ctx.base_url, treasury_id
        ))
        .header("Authorization", format!("Bearer {}", ctx.user_token))
        .json(&serde_json::json!({ "wallet_address": wallet_a }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let wallet_a_row = sqlx::query(
        "SELECT multisig_active, status FROM treasury_wallets WHERE treasury_id = $1 AND wallet_address = $2",
    )
    .bind(treasury_id)
    .bind(&wallet_a)
    .fetch_one(&ctx.db)
    .await
    .unwrap();
    assert!(wallet_a_row.get::<bool, _>("multisig_active"));
    assert_eq!(wallet_a_row.get::<String, _>("status"), "ACTIVE");

    let wallet_b_row = sqlx::query(
        "SELECT multisig_active, status FROM treasury_wallets WHERE treasury_id = $1 AND wallet_address = $2",
    )
    .bind(treasury_id)
    .bind(&wallet_b)
    .fetch_one(&ctx.db)
    .await
    .unwrap();
    assert!(!wallet_b_row.get::<bool, _>("multisig_active"));
    assert_eq!(wallet_b_row.get::<String, _>("status"), "PENDING");
}
