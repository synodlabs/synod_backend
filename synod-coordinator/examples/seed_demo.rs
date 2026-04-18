use chrono::Utc;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use uuid::Uuid;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await?;

    println!("Seeding demo data...");

    // 1. Create or Find Demo User
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (user_id, email, password_hash, name, role) 
         VALUES ($1, 'demo@synod.com', 'dummy', 'Demo User', 'ADMIN')
         ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email RETURNING user_id",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await?;

    let user_row = sqlx::query("SELECT user_id FROM users WHERE email = 'demo@synod.com'")
        .fetch_one(&pool)
        .await?;
    let actual_user_id: Uuid = user_row.get(0);

    // 2. Create Treasury
    let treasury_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO treasuries (treasury_id, owner_user_id, name, network, health, current_aum_usd, peak_aum_usd, constitution_version)
         VALUES ($1, $2, 'Primary Treasury', 'testnet', 'HEALTHY', 1250000.0, 1500000.0, 0)"
    )
    .bind(treasury_id)
    .bind(actual_user_id)
    .execute(&pool)
    .await?;

    // 3. Create Genesis Constitution
    let content = serde_json::json!({
        "agent_allocations": [],
        "memo": "Genesis Constitution",
        "max_drawdown_pct": 15.0,
        "governance_mode": "AUTO"
    });

    sqlx::query(
        "INSERT INTO constitution_history (treasury_id, version, state_hash, content, executed_at)
         VALUES ($1, 0, 'genesis', $2, $3)",
    )
    .bind(treasury_id)
    .bind(content)
    .bind(Utc::now())
    .execute(&pool)
    .await?;

    // 4. Add a Wallet
    let wallet_address = "GAX3B7F6J6W6E7M4Y...DEMO"; // Mock
    sqlx::query(
        "INSERT INTO treasury_wallets (treasury_id, wallet_address, label, multisig_active, status)
         VALUES ($1, $2, 'HOT_WALLET_01', true, 'ACTIVE')",
    )
    .bind(treasury_id)
    .bind(wallet_address)
    .execute(&pool)
    .await?;

    // 5. Create an Agent Slot
    let agent_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO agent_slots (agent_id, treasury_id, name, status, allocation_pct, tier_limit_usd, concurrent_permit_cap, wallet_address)
         VALUES ($1, $2, 'Sentinel Strategy', 'ACTIVE', 50.0, 5000.0, 3, $3)"
    )
    .bind(agent_id)
    .bind(treasury_id)
    .bind(wallet_address)
    .execute(&pool)
    .await?;

    println!("Demo data seeded successfully.");
    println!("Treasury ID: {}", treasury_id);
    println!("User ID: {}", actual_user_id);

    Ok(())
}
