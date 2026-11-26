//! Ending the setup wizard (audit 2026-11-25, TST-7): the flag a fresh installation's interface
//! asks for, kept over a restart.

use crate::common;

use axum::http::StatusCode;
use common::{get_json, post_json, test_router};

async fn completed(router: &axum::Router) -> bool {
    let (status, body) = get_json(router, "/api/v1/setup/status").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["wizard_completed"].as_bool().expect("a flag")
}

#[tokio::test]
async fn the_wizard_is_over_once_it_was_completed_and_stays_so() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    assert!(!completed(&router).await, "a fresh installation asks");

    let (status, body) = post_json(&router, "/api/v1/setup/complete", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "setup.wizard_completed", "{body}");
    assert!(completed(&router).await);

    let restarted = test_router(directory.path()).await;
    assert!(completed(&restarted).await, "kept over a restart");
}

/// Completing the setup is the administrator's: an installation with a login refuses anybody else.
#[tokio::test]
async fn nobody_completes_the_wizard_without_signing_in() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::auth_harness(directory.path()).await.router;
    let (status, body) = post_json(&router, "/api/v1/setup/complete", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}
