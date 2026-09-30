//! What an anonymous caller can do to the routes it is allowed to reach (security audit
//! 2026-09-30, findings 7 and 8): set the first password exactly once, and send nothing larger
//! than a sign-in needs.

use crate::common;

use axum::{
    body::Body,
    http::{StatusCode, header},
};
use common::{auth_harness, post_json, post_json_with_headers};

/// Every audit action the store holds, newest first.
async fn audit_actions(database: &rd_db::Database) -> Vec<String> {
    database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 500,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("query")
        .into_iter()
        .map(|record| record.action.as_str().to_owned())
        .collect()
}

/// Setup requests racing on a fresh installation: one wins, every other one is refused, and
/// the password in force is the winner's.
///
/// The check used to be a read followed by a write, so every request that read before the
/// first write landed also wrote, and the last password silently replaced the one the first
/// caller had just been told was set.
#[tokio::test]
async fn racing_setups_leave_exactly_one_password() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let passwords: Vec<String> = (0..8)
        .map(|index| format!("racing-password-{index}"))
        .collect();

    let answers = futures_util::future::join_all(passwords.iter().map(|password| {
        post_json(
            &harness.router,
            "/api/v1/auth/setup",
            serde_json::json!({ "password": password }),
        )
    }))
    .await;

    let winners: Vec<&String> = answers
        .iter()
        .zip(&passwords)
        .filter(|((status, _), _)| *status == StatusCode::OK)
        .map(|(_, password)| password)
        .collect();
    assert_eq!(winners.len(), 1, "{answers:?}");
    for (status, body) in answers
        .iter()
        .filter(|(status, _)| *status != StatusCode::OK)
    {
        assert_eq!(*status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["code"], "auth.setup_completed", "{body}");
    }

    // The winner signs in; two of the losers are enough to show theirs were not stored, and
    // stay below the sign-in limiter, which answers 429 once more than five have failed.
    let loser_sample = passwords
        .iter()
        .filter(|password| *password != winners[0])
        .take(2);
    for password in std::iter::once(winners[0]).chain(loser_sample) {
        let (status, body, _) = post_json_with_headers(
            &harness.router,
            "/api/v1/auth/login",
            serde_json::json!({ "password": password }),
        )
        .await;
        let expected = if password == winners[0] {
            StatusCode::OK
        } else {
            StatusCode::UNAUTHORIZED
        };
        assert_eq!(status, expected, "{password}: {body}");
    }

    // And the one setup that happened is on the record, once.
    let setups = audit_actions(&harness.database)
        .await
        .into_iter()
        .filter(|action| action == "setup_completed")
        .count();
    assert_eq!(setups, 1);
}

/// A password beyond any passphrase is refused before it is hashed.
#[tokio::test]
async fn an_endless_password_is_refused_at_setup() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/setup",
        serde_json::json!({ "password": "x".repeat(1025) }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "auth.password_too_long", "{body}");
    assert_eq!(body["params"]["max"], "1024", "{body}");
}

/// A route reachable without a credential takes a sign-in's worth of body, not an upload's.
///
/// The service-wide limit is 65 MiB for the NZB and container uploads, and it used to be the
/// only limit in front of the sign-in too: an anonymous caller could make the service read and
/// hash a password of tens of megabytes.
#[tokio::test]
async fn a_public_route_refuses_a_large_body() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let oversized = serde_json::json!({ "password": "x".repeat(rd_api::PUBLIC_BODY_LIMIT_BYTES) });

    for uri in ["/api/v1/auth/login", "/api/v1/auth/setup"] {
        let request = common::request_to("POST", uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(oversized.to_string()))
            .expect("request");
        let (status, body) = common::send(&harness.router, request).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{uri}: {body}");
    }

    // The ordinary sign-in still fits.
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/auth/login",
        serde_json::json!({ "password": "a-normal-password" }),
    )
    .await;
    assert_ne!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
}
