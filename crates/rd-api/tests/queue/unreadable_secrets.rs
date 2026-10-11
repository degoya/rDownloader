//! A stored credential the vault master key cannot open -- a data folder copied from another
//! machine or user account -- answers `secret.unreadable`, never `internal.error` (RD-1240-36).

use crate::common;

use axum::http::StatusCode;
use common::post_json;
use serde_json::json;

#[tokio::test]
async fn testing_a_server_whose_password_was_sealed_elsewhere_answers_the_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;
    let (status, created) = post_json(
        router,
        "/api/v1/usenet/servers",
        json!({
            "name": "moved-news",
            "host": "news.moved.test",
            "port": 563,
            "tls": true,
            "username": "reader",
            "password": "moved-news-password",
            "proxy_profile_id": null,
            "priority": 0,
            "max_connections": 2,
            "enabled": true,
            "clear_password": false,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("id");
    let references = harness.secrets.stored_references().await.expect("vault");
    assert_eq!(references.len(), 1, "{references:?}");
    let reference = &references[0];

    // The same entry as another installation's vault, with its own master key, sealed it.
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let other = rd_secrets::SecretStore::open(elsewhere.path().to_owned())
        .await
        .expect("other vault");
    other
        .put_at(
            reference,
            rd_secrets::SecretString::from("moved-news-password".to_owned()),
        )
        .await
        .expect("sealed elsewhere");
    let file = format!("{}.secret", reference.trim_start_matches("vault://"));
    std::fs::copy(
        elsewhere.path().join(&file),
        directory.path().join("secrets").join(&file),
    )
    .expect("copy the entry over");

    let (status, body) = post_json(
        router,
        &format!("/api/v1/usenet/servers/{id}/test"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], rd_secrets::SECRET_UNREADABLE, "{body}");
    let text = body.to_string();
    assert!(!text.contains("vault://"), "{text}");
    assert!(!text.contains("moved-news-password"), "{text}");
}
