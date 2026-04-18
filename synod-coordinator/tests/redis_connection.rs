use redis::AsyncCommands;

#[serial_test::serial]
#[tokio::test]
async fn redis_connection_works() {
    dotenvy::dotenv().ok();
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());

    let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
    let mut conn = redis::aio::ConnectionManager::new(client)
        .await
        .expect("Failed to connect to Redis");

    // SET and GET a test key
    let _: () = conn
        .set("synod:test:ping", "pong")
        .await
        .expect("Failed to SET test key");

    let val: String = conn
        .get("synod:test:ping")
        .await
        .expect("Failed to GET test key");

    assert_eq!(val, "pong");

    // Clean up
    let _: () = conn
        .del("synod:test:ping")
        .await
        .expect("Failed to DEL test key");
}

#[serial_test::serial]
#[tokio::test]
async fn redis_connection_manager_pooling() {
    dotenvy::dotenv().ok();
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6379".to_string());

    let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
    let manager = redis::aio::ConnectionManager::new(client)
        .await
        .expect("Failed to create ConnectionManager");

    // Verify we can clone and use the manager concurrently (connection pooling)
    let mut m1 = manager.clone();
    let mut m2 = manager.clone();

    let _: () = m1.set("synod:test:pool1", "a").await.unwrap();
    let _: () = m2.set("synod:test:pool2", "b").await.unwrap();

    let v1: String = m1.get("synod:test:pool1").await.unwrap();
    let v2: String = m2.get("synod:test:pool2").await.unwrap();

    assert_eq!(v1, "a");
    assert_eq!(v2, "b");

    // Clean up
    let _: () = m1.del("synod:test:pool1").await.unwrap();
    let _: () = m2.del("synod:test:pool2").await.unwrap();
}
