//! A call limit per API token (RD-1200-04, `docs/security/mcp.md` finding 8): minted with the
//! token or set later, the call above it is refused with `429 api.token_rate_limited` and a
//! `Retry-After`, a raised or cleared limit admits again at once, and every change is audited.

use crate::common;

use axum::http::StatusCode;
use common::{API_BEARER, auth_harness, get_with_bearer, post_with_bearer, put_with_bearer};

const ADMIN_PASSWORD: &str = "correct-horse-battery";

/// Mints an `api:read` token with `calls_per_minute` through the route and returns its bearer
/// and id.
async fn mint(router: &axum::Router, label: &str, calls_per_minute: u32) -> (String, String) {
    let (status, minted) = post_with_bearer(
        router,
        "/api/v1/api-tokens",
        API_BEARER,
        serde_json::json!({
            "label": label,
            "scopes": [rd_core::API_READ_SCOPE],
            "calls_per_minute": calls_per_minute,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    assert_eq!(
        minted["token"]["calls_per_minute"], calls_per_minute,
        "{minted}"
    );
    (
        minted["bearer"].as_str().expect("bearer").to_owned(),
        minted["token"]["id"].as_str().expect("id").to_owned(),
    )
}

#[tokio::test]
async fn the_call_above_the_limit_is_refused_and_a_raised_limit_admits_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    common::sign_in(&harness.router, ADMIN_PASSWORD).await;
    let (bearer, id) = mint(&harness.router, "limited-three", 3).await;

    for call in 1..=3 {
        let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", &bearer).await;
        assert_eq!(status, StatusCode::OK, "call {call}: {body}");
    }
    let refused = common::send_raw(
        &harness.router,
        common::request_to("GET", "/api/v1/downloads")
            .header("authorization", format!("Bearer {bearer}"))
            .body(axum::body::Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(refused.0, StatusCode::TOO_MANY_REQUESTS);
    let wait: u64 = refused
        .1
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .expect("a Retry-After");
    assert!((1..=60).contains(&wait), "{wait}");
    let body: serde_json::Value = serde_json::from_slice(&refused.2).expect("json");
    assert_eq!(body["code"], "api.token_rate_limited", "{body}");
    assert_eq!(body["params"]["limit"], "3", "{body}");

    // Another token is not counted with this one.
    let (status, _) = get_with_bearer(&harness.router, "/api/v1/downloads", API_BEARER).await;
    assert_eq!(status, StatusCode::OK);

    // Raised within the same minute: the window keeps its three calls, the fourth is admitted.
    let (status, updated) = put_with_bearer(
        &harness.router,
        &format!("/api/v1/api-tokens/{id}/limits"),
        API_BEARER,
        serde_json::json!({ "calls_per_minute": 4 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["calls_per_minute"], 4, "{updated}");
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", &bearer).await;
    assert_eq!(status, StatusCode::OK, "the raised limit admits: {body}");
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", &bearer).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");

    // Cleared: no limit at all.
    let (status, cleared) = put_with_bearer(
        &harness.router,
        &format!("/api/v1/api-tokens/{id}/limits"),
        API_BEARER,
        serde_json::json!({ "calls_per_minute": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert!(cleared["calls_per_minute"].is_null(), "{cleared}");
    for call in 0..10 {
        let (status, body) = get_with_bearer(&harness.router, "/api/v1/downloads", &bearer).await;
        assert_eq!(status, StatusCode::OK, "unlimited call {call}: {body}");
    }

    let changes = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::TokenLimitsChanged),
            limit: 10,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!(changes[0].details["calls_per_minute_before"], "4");
    assert_eq!(changes[0].details["calls_per_minute_after"], "none");
    assert_eq!(changes[1].details["calls_per_minute_before"], "3");
    assert_eq!(changes[1].details["calls_per_minute_after"], "4");
}

#[tokio::test]
async fn a_limit_out_of_range_is_refused_and_an_unknown_token_is_not_found() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    common::sign_in(&harness.router, ADMIN_PASSWORD).await;
    for limit in [0, 6001] {
        let (status, body) = post_with_bearer(
            &harness.router,
            "/api/v1/api-tokens",
            API_BEARER,
            serde_json::json!({ "label": "out-of-range", "calls_per_minute": limit }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{limit}: {body}");
        assert_eq!(body["code"], "api.token_rate_range", "{body}");
    }
    let (status, body) = put_with_bearer(
        &harness.router,
        "/api/v1/api-tokens/0192f0c4-0000-7000-8000-00000000abcd/limits",
        API_BEARER,
        serde_json::json!({ "calls_per_minute": 10 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "api.token_not_found", "{body}");
}

/// The audit record of a REST call says it came through REST (RD-1200-04); the MCP half of
/// the comparison is `mcp::everything::audit_channel`.
#[tokio::test]
async fn a_rest_call_is_audited_as_rest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    common::sign_in(&harness.router, ADMIN_PASSWORD).await;
    let (_, id) = mint(&harness.router, "audited-rest", 100).await;
    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            action: Some(rd_core::AuditAction::TokenCreated),
            target_id: Some(id),
            limit: 10,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    let created = records.first().expect("a token_created record");
    assert_eq!(created.actor_kind, rd_core::AuditActorKind::Token);
    assert_eq!(created.via, rd_core::AuditChannel::Rest);
    assert_eq!(created.details["calls_per_minute"], "100");
}
