use axum::http::StatusCode;
use serde_json::{json, Value};

mod common;
use common::spawn_test_server;

#[serial_test::serial]
#[tokio::test]
async fn test_email_password_flow() {
    let base_url = spawn_test_server().await;
    let client = reqwest::Client::new();

    let email = "test@example.com";
    let password = "super_secret_password";

    // 1. Register
    let reg_res = client
        .post(&format!("{}/v1/auth/register", base_url))
        .json(&json!({ "email": email, "password": password }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg_res.status(), StatusCode::OK);

    let reg_body: Value = reg_res.json().await.unwrap();
    assert!(
        reg_body.get("token").is_some(),
        "Registration should return a JWT"
    );

    // 2. Login
    let login_res = client
        .post(&format!("{}/v1/auth/login", base_url))
        .json(&json!({ "email": email, "password": password }))
        .send()
        .await
        .unwrap();
    assert_eq!(login_res.status(), StatusCode::OK);
    let login_body: Value = login_res.json().await.unwrap();
    assert!(
        login_body.get("token").is_some(),
        "Login should return a JWT"
    );
}

#[serial_test::serial]
#[tokio::test]
async fn test_rate_limiting_11th_attempt_fails() {
    let base_url = spawn_test_server().await;
    let client = reqwest::Client::new();
    let email = "rate_limit@example.com";

    // Rate limit prefix triggers on login, not register
    for i in 1..=11 {
        let res = client
            .post(&format!("{}/v1/auth/login", base_url))
            .json(&json!({ "email": email, "password": "wrongpassword" }))
            .send()
            .await
            .unwrap();

        if i <= 10 {
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        } else {
            assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        }
    }
}

#[serial_test::serial]
#[tokio::test]
async fn test_passkey_flow() {
    let base_url = spawn_test_server().await;
    let client = reqwest::Client::new();
    let email = "passkey_user@example.com";

    // 1. Register Begin
    let begin_res = client
        .post(&format!("{}/v1/auth/passkey/register/begin", base_url))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap();
    assert_eq!(begin_res.status(), StatusCode::OK);
    let begin_body: Value = begin_res.json().await.unwrap();
    let challenge = begin_body.get("challenge").unwrap().as_str().unwrap();

    // 2. Register Complete
    let complete_res = client
        .post(&format!("{}/v1/auth/passkey/register/complete", base_url))
        .json(&json!({ "email": email, "challenge": challenge, "credential_id": "mock_cred" }))
        .send()
        .await
        .unwrap();
    assert_eq!(complete_res.status(), StatusCode::OK);
    let complete_body: Value = complete_res.json().await.unwrap();
    assert!(complete_body.get("token").is_some());

    // 3. Login Begin
    let login_begin_res = client
        .post(&format!("{}/v1/auth/passkey/login/begin", base_url))
        .json(&json!({ "email": email }))
        .send()
        .await
        .unwrap();
    assert_eq!(login_begin_res.status(), StatusCode::OK);
    let login_begin_body: Value = login_begin_res.json().await.unwrap();
    let login_challenge = login_begin_body.get("challenge").unwrap().as_str().unwrap();

    // 4. Login Complete
    let login_complete_res = client
        .post(&format!("{}/v1/auth/passkey/login/complete", base_url))
        .json(
            &json!({ "email": email, "challenge": login_challenge, "credential_id": "mock_cred" }),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(login_complete_res.status(), StatusCode::OK);
    let login_complete_body: Value = login_complete_res.json().await.unwrap();
    assert!(login_complete_body.get("token").is_some());
}
