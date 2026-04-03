use sqlx::PgPool;

#[sqlx::test]
async fn migrations_apply_cleanly(pool: PgPool) {
    // If we get here, all migrations in migrations/ ran successfully.
    // Verify core tables exist by querying information_schema.
    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT table_name::text FROM information_schema.tables 
         WHERE table_schema = 'public' 
         ORDER BY table_name"
    )
    .fetch_all(&pool)
    .await
    .expect("Failed to query information_schema");

    let table_names: Vec<&str> = tables.iter().map(|t| t.0.as_str()).collect();

    // Verify all 10 Appendix A tables exist
    assert!(table_names.contains(&"users"), "Missing table: users");
    assert!(table_names.contains(&"user_passkeys"), "Missing table: user_passkeys");
    assert!(table_names.contains(&"wallet_connections"), "Missing table: wallet_connections");
    assert!(table_names.contains(&"treasuries"), "Missing table: treasuries");
    assert!(table_names.contains(&"treasury_wallets"), "Missing table: treasury_wallets");
    assert!(table_names.contains(&"constitution_history"), "Missing table: constitution_history");
    assert!(table_names.contains(&"agent_slots"), "Missing table: agent_slots");
    assert!(table_names.contains(&"permit_groups"), "Missing table: permit_groups");
    assert!(table_names.contains(&"permits"), "Missing table: permits");
    assert!(table_names.contains(&"events"), "Missing table: events");
    assert!(table_names.contains(&"halt_log"), "Missing table: halt_log");
}

#[sqlx::test]
async fn users_table_has_correct_columns(pool: PgPool) {
    // Verify the users table schema matches spec
    let columns: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name::text FROM information_schema.columns 
         WHERE table_name = 'users' 
         ORDER BY ordinal_position"
    )
    .fetch_all(&pool)
    .await
    .expect("Failed to query users columns");

    let col_names: Vec<&str> = columns.iter().map(|c| c.0.as_str()).collect();
    assert!(col_names.contains(&"user_id"));
    assert!(col_names.contains(&"email"));
    assert!(col_names.contains(&"password_hash"));
    assert!(col_names.contains(&"created_at"));
    assert!(col_names.contains(&"last_seen"));
    assert!(col_names.contains(&"is_active"));
}

#[sqlx::test]
async fn permits_table_has_indexes(pool: PgPool) {
    let indexes: Vec<(String,)> = sqlx::query_as(
        "SELECT indexname::text FROM pg_indexes 
         WHERE tablename = 'permits' 
         ORDER BY indexname"
    )
    .fetch_all(&pool)
    .await
    .expect("Failed to query permit indexes");

    let idx_names: Vec<&str> = indexes.iter().map(|i| i.0.as_str()).collect();
    assert!(idx_names.contains(&"idx_permits_agent_status"), "Missing index: idx_permits_agent_status");
    assert!(idx_names.contains(&"idx_permits_treasury_status"), "Missing index: idx_permits_treasury_status");
}
