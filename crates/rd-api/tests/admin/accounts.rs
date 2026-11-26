//! Changing and deleting a provider account (audit 2026-11-25, TST-7). Creating one, its test and
//! its credentials' way into a backup have suites of their own.

use crate::common;

use axum::http::StatusCode;
use common::{delete_json, get_json, put_json, test_harness};
use serde_json::{Value, json};

/// A provider no installed plugin knows: an account keeps working as a row after its plugin went.
const PROVIDER: &str = "plugin-went-away";

fn account(label: &str) -> Value {
    json!({
        "provider": PROVIDER,
        "label": label,
        "username": "somebody",
        "secret": null,
        "cookies": null,
        "clear_secret": false,
        "clear_cookies": false,
        "proxy_profile_id": null,
        "enabled": true,
    })
}

#[tokio::test]
async fn an_account_is_changed_keeps_its_secret_and_is_deleted_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let router = &harness.router;
    let secret_ref = harness
        .secrets
        .put_string("kept-account-secret".to_owned())
        .await
        .expect("vault");
    let id = harness
        .database
        .create_account(rd_db::NewAccount {
            provider: PROVIDER.to_owned(),
            label: "before".to_owned(),
            username: None,
            credential_mode: None,
            secret_ref: Some(secret_ref),
            cookie_ref: None,
            proxy_profile_id: None,
            enabled: true,
        })
        .await
        .expect("account")
        .id;
    let uri = format!("/api/v1/accounts/{id}");

    // Without a secret in the request the stored one stays, and no answer carries it.
    let mut changed = account("after");
    changed["enabled"] = json!(false);
    let (status, body) = put_json(router, &uri, changed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["label"], "after", "{body}");
    assert_eq!(body["username"], "somebody", "{body}");
    assert_eq!(body["enabled"], false, "{body}");
    assert_eq!(body["has_secret"], true, "{body}");
    assert!(!body.to_string().contains("kept-account-secret"), "{body}");

    let mut cleared = account("after");
    cleared["clear_secret"] = json!(true);
    let (status, body) = put_json(router, &uri, cleared).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["has_secret"], false, "{body}");

    let (status, body) = delete_json(router, &uri).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "account.deleted", "{body}");
    let (_, accounts) = get_json(router, "/api/v1/accounts").await;
    assert_eq!(accounts, json!([]));

    let (status, body) = delete_json(router, &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "account.not_found", "{body}");
}
