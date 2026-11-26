//! Starting an account's sign-in and trying an auth profile (audit 2026-11-25, TST-7), as far as
//! neither reaches a provider: what is refused before anything is sent, and under which code.
//! The flows themselves run against mock providers (`oauth_callback`, the plugin contract tests).

use crate::common;

use axum::http::StatusCode;
use common::{post_json, test_harness};
use serde_json::json;

#[tokio::test]
async fn a_sign_in_needs_the_account_and_a_plugin_for_its_provider() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let unknown = rd_core::AccountId::new();
    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/accounts/{unknown}/auth/begin"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "account.not_found", "{body}");

    let account = harness
        .database
        .create_account(rd_db::NewAccount {
            provider: "nobody-signs-in-here".to_owned(),
            label: "sign-in probe".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: None,
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id;
    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/accounts/{account}/auth/begin"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "account.auth_unavailable", "{body}");
}

#[tokio::test]
async fn an_unknown_or_expired_auth_profile_is_not_tried() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;

    let unknown = rd_core::AuthProfileId::new();
    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/auth-profiles/{unknown}/test"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    // A session past its end would only be refused by the site, which the person reads as "wrong".
    let secret_ref = harness
        .secrets
        .put_string("expired-session-token".to_owned())
        .await
        .expect("vault");
    let profile = harness
        .database
        .create_auth_profile(rd_db::NewAuthProfile {
            name: "expired".to_owned(),
            scope: rd_core::AuthScope::parse("expired.example.invalid", false).expect("scope"),
            method: rd_core::AuthMethod::Bearer,
            origin: rd_core::AuthOrigin::Manual,
            enabled: true,
            expires_at: Some(rd_core::Timestamp::now() - rd_core::SignedDuration::from_hours(1)),
            username: None,
            secret_ref: Some(secret_ref),
            certificate_ref: None,
        })
        .await
        .expect("profile")
        .id;
    let (status, body) = post_json(
        &harness.router,
        &format!("/api/v1/auth-profiles/{profile}/test"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "authprofile.expired", "{body}");
}
