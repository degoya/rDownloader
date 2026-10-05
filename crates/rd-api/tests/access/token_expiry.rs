//! Token expiry (RD-1110-07, audit S13): an API or capture token past its expiry is refused
//! exactly like a revoked one, with the same 401 and code, and minting takes the expiry as a
//! number of days or none at all.

use crate::common;

use axum::http::StatusCode;
use common::{auth_harness, get_with_bearer};
use sha2::{Digest, Sha256};

const ADMIN_PASSWORD: &str = "correct-horse-battery";

/// Mints a token straight into the store, expiring `minutes` from now (negative: already past).
async fn token_expiring(
    database: &rd_db::Database,
    bearer: &str,
    scope: &str,
    minutes: i64,
) -> rd_core::CaptureToken {
    database
        .create_expiring_capture_token(
            rd_core::CaptureTokenId::new(),
            format!("expiry {bearer}"),
            hex::encode(Sha256::digest(bearer.as_bytes())),
            vec![scope.to_owned()],
            Some(chrono::Utc::now() + chrono::Duration::minutes(minutes)),
        )
        .await
        .expect("token")
}

#[tokio::test]
async fn an_expired_api_token_is_refused_like_one_never_issued() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    // Before the first-run setup every bearer meets `auth.setup_pending`; the rule under test is
    // the one of a configured installation.
    common::sign_in(&harness.router, ADMIN_PASSWORD).await;
    token_expiring(
        &harness.database,
        "expiry-past-read",
        rd_core::API_READ_SCOPE,
        -1,
    )
    .await;
    token_expiring(
        &harness.database,
        "expiry-future-read",
        rd_core::API_READ_SCOPE,
        60,
    )
    .await;

    let (status, body) =
        get_with_bearer(&harness.router, "/api/v1/downloads", "expiry-future-read").await;
    assert_eq!(status, StatusCode::OK, "a token before its expiry: {body}");

    let (status, expired) =
        get_with_bearer(&harness.router, "/api/v1/downloads", "expiry-past-read").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{expired}");
    assert_eq!(expired["code"], "auth.session_required", "{expired}");
    let (_, unknown) =
        get_with_bearer(&harness.router, "/api/v1/downloads", "expiry-never-issued").await;
    assert_eq!(
        expired["code"], unknown["code"],
        "an expired token answers like a token that does not exist"
    );
}

#[tokio::test]
async fn an_expired_capture_token_is_refused_with_the_capture_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    token_expiring(
        &harness.database,
        "expiry-past-capture",
        rd_core::CAPTURE_SCOPE,
        -1,
    )
    .await;
    token_expiring(
        &harness.database,
        "expiry-future-capture",
        rd_core::CAPTURE_SCOPE,
        60,
    )
    .await;

    let (status, body) = get_with_bearer(
        &harness.router,
        "/api/v1/capture/ping",
        "expiry-future-capture",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, expired) = get_with_bearer(
        &harness.router,
        "/api/v1/capture/ping",
        "expiry-past-capture",
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{expired}");
    assert_eq!(expired["code"], "capture.token_required", "{expired}");
}

/// The expiry stays visible: an expired token is still listed with its date until somebody
/// revokes it, and minting stores the days asked for, or none.
#[tokio::test]
async fn minting_takes_an_expiry_in_days_and_lists_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let session = common::sign_in(&harness.router, ADMIN_PASSWORD).await;

    let (status, never) = common::post_json_with_cookie(
        &harness.router,
        "/api/v1/api-tokens",
        &session,
        serde_json::json!({ "label": "Forever", "scopes": ["api:read"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{never}");
    assert!(never["token"]["expires_at"].is_null(), "{never}");

    let (status, monthly) = common::post_json_with_cookie(
        &harness.router,
        "/api/v1/api-tokens",
        &session,
        serde_json::json!({ "label": "Monthly", "scopes": ["api:read"], "expires_in_days": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{monthly}");
    let expires_at: chrono::DateTime<chrono::Utc> = monthly["token"]["expires_at"]
        .as_str()
        .expect("an expiry")
        .parse()
        .expect("a timestamp");
    let ahead = expires_at - chrono::Utc::now();
    assert!(
        ahead > chrono::Duration::days(29) && ahead <= chrono::Duration::days(30),
        "{ahead}"
    );

    for days in [0, 3651] {
        let (status, refused) = common::post_json_with_cookie(
            &harness.router,
            "/api/v1/api-tokens",
            &session,
            serde_json::json!({ "label": "Odd", "scopes": ["api:read"], "expires_in_days": days }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{days}: {refused}");
        assert_eq!(refused["code"], "api.token_expiry_range", "{days}");
    }

    let (status, paired) = common::post_json_with_cookie(
        &harness.router,
        "/api/v1/capture/pair",
        &session,
        serde_json::json!({ "label": "Laptop", "expires_in_days": 7 }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{paired}");
    assert!(paired["token"]["expires_at"].is_string(), "{paired}");

    let lapsed = token_expiring(
        &harness.database,
        "expiry-lapsed-listed",
        rd_core::API_READ_SCOPE,
        -1,
    )
    .await;
    let (status, listing) =
        common::get_with_cookie(&harness.router, "/api/v1/api-tokens", &session).await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    let listed = listing
        .as_array()
        .expect("a list")
        .iter()
        .find(|entry| entry["id"] == lapsed.id.to_string())
        .unwrap_or_else(|| panic!("the expired token is listed until revoked: {listing}"));
    assert!(listed["expires_at"].is_string(), "{listed}");
}
