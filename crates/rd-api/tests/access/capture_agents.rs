//! The list of paired capture agents and taking one's token back (audit 2026-11-25, TST-7).
//!
//! The revocation was tested at the database and for API tokens, never through the route the
//! settings page calls: what it has to do is end the agent's access at once and that agent's
//! alone.

use crate::common;

use axum::http::StatusCode;
use common::{API_BEARER, CAPTURE_BEARER, READ_BEARER, auth_harness, get_with_bearer};
use serde_json::json;

const AGENTS: &str = "/api/v1/capture/agents";
const PING: &str = "/api/v1/capture/ping";

/// Pairs an agent as the settings page does; answers its bearer and the token's id.
async fn pair(router: &axum::Router, label: &str) -> (String, String) {
    let (status, paired) = common::post_with_bearer(
        router,
        "/api/v1/capture/pair",
        API_BEARER,
        json!({ "label": label }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{paired}");
    let text = |value: &serde_json::Value| value.as_str().expect("a string").to_owned();
    (text(&paired["bearer"]), text(&paired["token"]["id"]))
}

async fn listed(router: &axum::Router) -> Vec<String> {
    let (status, agents) = get_with_bearer(router, AGENTS, API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{agents}");
    agents
        .as_array()
        .expect("a list")
        .iter()
        .map(|agent| agent["label"].as_str().expect("a label").to_owned())
        .collect()
}

#[tokio::test]
async fn a_revoked_agent_is_refused_at_once_and_the_others_keep_working() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = auth_harness(directory.path()).await.router;
    let (lost, lost_id) = pair(&router, "lost laptop").await;
    let (kept, _) = pair(&router, "desk").await;
    for bearer in [&lost, &kept] {
        let (status, body) = get_with_bearer(&router, PING, bearer).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let labels = listed(&router).await;
    for label in ["lost laptop", "desk"] {
        assert!(labels.iter().any(|entry| entry == label), "{labels:?}");
    }

    let uri = format!("/api/v1/capture/agents/{lost_id}");
    let (status, body) = common::request_with_bearer(&router, "DELETE", &uri, API_BEARER).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "capture.revoked", "{body}");

    let (status, body) = get_with_bearer(&router, PING, &lost).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], "capture.token_required", "{body}");
    let (status, body) = common::post_with_bearer(
        &router,
        "/api/v1/capture/batches",
        &lost,
        json!({ "text": "https://files.example.com/late.bin", "source": "browser_extension" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (status, body) = get_with_bearer(&router, PING, &kept).await;
    assert_eq!(status, StatusCode::OK, "the other agent: {body}");
    let labels = listed(&router).await;
    assert!(
        !labels.iter().any(|entry| entry == "lost laptop"),
        "{labels:?}"
    );
    assert!(labels.iter().any(|entry| entry == "desk"), "{labels:?}");

    // Taken back once, it is gone: a second time there is nothing to take.
    let (status, body) = common::request_with_bearer(&router, "DELETE", &uri, API_BEARER).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "capture.not_found", "{body}");
}

#[tokio::test]
async fn an_agent_and_a_reader_can_neither_list_nor_revoke_agents() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = auth_harness(directory.path()).await.router;
    let (victim, id) = pair(&router, "victim").await;
    let uri = format!("/api/v1/capture/agents/{id}");

    for bearer in [CAPTURE_BEARER, READ_BEARER] {
        for (method, uri) in [("GET", AGENTS), ("DELETE", uri.as_str())] {
            let (status, body) = common::request_with_bearer(&router, method, uri, bearer).await;
            assert!(
                matches!(status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED),
                "{method} {uri}: {status} {body}"
            );
        }
    }
    let (status, body) = get_with_bearer(&router, PING, &victim).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A route names the rows it deletes by id: one it does not know is `404`, not a success.
#[tokio::test]
async fn an_unknown_agent_is_not_found() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let uri = format!("/api/v1/capture/agents/{}", rd_core::CaptureTokenId::new());
    let (status, body) = common::delete_json(&router, &uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "capture.not_found", "{body}");
}
