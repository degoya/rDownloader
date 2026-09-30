//! Changing how the account signs in takes a signed-in administrator and the password again
//! (security audit 2026-09-30, finding 2).
//!
//! The routes cost `api:secrets`, and a passkey signs in without the password: a token holding
//! the credentials area used to be able to enrol a passkey of its own and open a full session
//! with it. These tests hold both halves of the fix — no bearer token, whatever its areas, and
//! no enrolment or removal without the password.

use crate::common;

use axum::{
    body::Body,
    http::{StatusCode, header},
};
use common::{Options, get_with_cookie, post_json_with_cookie, post_with_bearer, sign_in};

const PASSWORD: &str = "correct-horse-battery";
const SECRETS_BEARER: &str = "step-up-secrets-bearer";

async fn harness(directory: &std::path::Path) -> common::Harness {
    common::harness(
        directory,
        Options::default()
            .login()
            .token(SECRETS_BEARER, rd_core::API_SECRETS_SCOPE),
    )
    .await
}

/// `DELETE /api/v1/mfa/credentials/{id}` with a session and the password it was given.
async fn remove(
    harness: &common::Harness,
    session: &str,
    id: &str,
    password: &str,
) -> (StatusCode, serde_json::Value) {
    let request = common::request_to("DELETE", &format!("/api/v1/mfa/credentials/{id}"))
        .header(header::COOKIE, format!("rd_session={session}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "password": password }).to_string(),
        ))
        .expect("request");
    common::send(&harness.router, request).await
}

/// The audit records of one action, newest first, as `(outcome, stage)`.
async fn records_of(database: &rd_db::Database, action: &str) -> Vec<(String, Option<String>)> {
    database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 500,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("query")
        .into_iter()
        .filter(|record| record.action.as_str() == action)
        .map(|record| {
            (
                record.outcome.as_str().to_owned(),
                record.details.get("stage").cloned(),
            )
        })
        .collect()
}

/// A credentials token, with the right password in hand, still may not add a way in.
#[tokio::test]
async fn a_credentials_token_cannot_change_the_second_factors() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    sign_in(&harness.router, PASSWORD).await;

    for uri in [
        "/api/v1/mfa/passkey",
        "/api/v1/mfa/totp",
        "/api/v1/mfa/disable",
        "/api/v1/mfa/recovery-codes",
        "/api/v1/mfa/passkey/confirm",
    ] {
        let (status, body) = post_with_bearer(
            &harness.router,
            uri,
            SECRETS_BEARER,
            serde_json::json!({
                "password": PASSWORD,
                "ceremony_id": "x",
                "credential": {},
            }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
        assert_eq!(body["code"], "mfa.session_required", "{uri}: {body}");
    }

    let request = common::request_to(
        "DELETE",
        "/api/v1/mfa/credentials/00000000-0000-7000-8000-000000000000",
    )
    .header(header::AUTHORIZATION, format!("Bearer {SECRETS_BEARER}"))
    .header(header::CONTENT_TYPE, "application/json")
    .body(Body::from(
        serde_json::json!({ "password": PASSWORD }).to_string(),
    ))
    .expect("request");
    let (status, body) = common::send(&harness.router, request).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "mfa.session_required", "{body}");

    // Nothing was enrolled on the way.
    let (_, status_body) = get_with_bearer_status(&harness).await;
    assert_eq!(
        status_body["credentials"],
        serde_json::json!([]),
        "{status_body}"
    );
    // And the refusal is on the record.
    assert!(
        records_of(&harness.database, "mfa_enrolled")
            .await
            .contains(&("failure".to_owned(), Some("session".to_owned())))
    );
}

async fn get_with_bearer_status(harness: &common::Harness) -> (StatusCode, serde_json::Value) {
    common::get_with_bearer(&harness.router, "/api/v1/mfa", SECRETS_BEARER).await
}

/// A session alone is not enough to add a factor: the password is asked for again.
#[tokio::test]
async fn enrolling_takes_the_password_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let session = sign_in(&harness.router, PASSWORD).await;

    let (status, body) = post_json_with_cookie(
        &harness.router,
        "/api/v1/mfa/totp",
        &session,
        serde_json::json!({ "label": "Phone", "password": "not-the-password" }),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], "auth.invalid_credentials", "{body}");

    let request = common::request_to("POST", "/api/v1/mfa/passkey")
        .header(header::COOKIE, format!("rd_session={session}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "password": "not-the-password" }).to_string(),
        ))
        .expect("request");
    let (status, body) = common::send(&harness.router, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["code"], "auth.invalid_credentials", "{body}");

    let (_, listing) = get_with_cookie(&harness.router, "/api/v1/mfa", &session).await;
    assert_eq!(listing["credentials"], serde_json::json!([]), "{listing}");
    assert!(
        records_of(&harness.database, "mfa_enrolled")
            .await
            .contains(&("failure".to_owned(), Some("password".to_owned())))
    );
}

/// Removing a factor takes the password too, and both ends of its life are on the record.
#[tokio::test]
async fn removing_takes_the_password_again_and_both_ends_are_recorded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = harness(directory.path()).await;
    let session = sign_in(&harness.router, PASSWORD).await;

    let (status, enrolled) = post_json_with_cookie(
        &harness.router,
        "/api/v1/mfa/totp",
        &session,
        serde_json::json!({ "label": "Phone", "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{enrolled}");
    let id = enrolled["credential_id"].as_str().expect("id").to_owned();
    let secret = enrolled["secret"].as_str().expect("secret").to_owned();
    let (status, body) = post_json_with_cookie(
        &harness.router,
        &format!("/api/v1/mfa/totp/{id}/confirm"),
        &session,
        serde_json::json!({ "code": crate::mfa::current_code(&secret) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        records_of(&harness.database, "mfa_enrolled")
            .await
            .contains(&("success".to_owned(), None))
    );

    let (status, body) = remove(&harness, &session, &id, "not-the-password").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    let (_, listing) = get_with_cookie(&harness.router, "/api/v1/mfa", &session).await;
    assert_eq!(
        listing["credentials"].as_array().map(Vec::len),
        Some(1),
        "a wrong password removed the factor: {listing}"
    );

    let (status, body) = remove(&harness, &session, &id, PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, listing) = get_with_cookie(&harness.router, "/api/v1/mfa", &session).await;
    assert_eq!(listing["credentials"], serde_json::json!([]), "{listing}");
    let removed = records_of(&harness.database, "mfa_removed").await;
    assert!(
        removed.contains(&("success".to_owned(), None)),
        "{removed:?}"
    );
    assert!(
        removed.contains(&("failure".to_owned(), Some("password".to_owned()))),
        "{removed:?}"
    );
}
