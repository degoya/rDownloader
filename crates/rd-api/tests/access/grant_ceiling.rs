//! A credential hands out no more than it holds (security audit 2026-09-30, finding 1).
//!
//! Minting and re-scoping a token cost `api:secrets`, and `api:secrets` confers nothing else.
//! Without a ceiling, a token holding only the credentials area could mint `api:*`, or re-scope
//! itself to it, and so reach administration through the one area that was meant to stop short
//! of it.

use crate::common;

use axum::http::StatusCode;
use common::{auth_harness, get_with_bearer, patch_with_bearer, post_with_bearer, sign_in};
use sha2::{Digest, Sha256};

const PASSWORD: &str = "correct-horse-battery";

/// Mints a bearer holding exactly `scopes`, straight into the store.
async fn bearer_holding(database: &rd_db::Database, label: &str, scopes: &[&str]) -> String {
    let bearer = format!("grant-ceiling-{label}");
    database
        .create_capture_token(
            rd_core::CaptureTokenId::new(),
            label.to_owned(),
            hex::encode(Sha256::digest(bearer.as_bytes())),
            scopes.iter().map(|scope| (*scope).to_owned()).collect(),
        )
        .await
        .expect("token");
    bearer
}

async fn mint(
    harness: &common::Harness,
    bearer: &str,
    scopes: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    post_with_bearer(
        &harness.router,
        "/api/v1/api-tokens",
        bearer,
        serde_json::json!({ "label": "Minted", "scopes": scopes }),
    )
    .await
}

#[tokio::test]
async fn a_credentials_token_cannot_mint_an_area_it_does_not_hold() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let bearer = bearer_holding(&harness.database, "secrets", &["api:secrets"]).await;

    for (scopes, missing) in [
        (serde_json::json!(["api:*"]), None),
        (serde_json::json!(["api:admin"]), None),
        // The island implies nothing, so the refusal names exactly it.
        (serde_json::json!(["api:metrics"]), Some("api:metrics")),
        (serde_json::json!(["api:config"]), None),
        (serde_json::json!(["api:secrets", "api:queue"]), None),
    ] {
        let (status, body) = mint(&harness, &bearer, scopes.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{scopes}: {body}");
        assert_eq!(body["code"], "api.scope_exceeds_grant", "{scopes}: {body}");
        if let Some(missing) = missing {
            assert_eq!(body["params"]["scope"], missing, "{body}");
        }
    }

    // What it does hold, it may hand on.
    let (status, body) = mint(&harness, &bearer, serde_json::json!(["api:secrets"])).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

/// Implications count on both sides: holding `api:queue` means holding reading too, so a
/// queue token may mint a reading one, and a queue token alone may not mint configuration.
#[tokio::test]
async fn implied_areas_count_on_both_sides() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let bearer = bearer_holding(
        &harness.database,
        "secrets-queue",
        &["api:secrets", "api:queue"],
    )
    .await;

    for allowed in [
        serde_json::json!(["api:read"]),
        serde_json::json!(["api:queue"]),
    ] {
        let (status, body) = mint(&harness, &bearer, allowed.clone()).await;
        assert_eq!(status, StatusCode::CREATED, "{allowed}: {body}");
    }
    let (status, body) = mint(&harness, &bearer, serde_json::json!(["api:config"])).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "api.scope_exceeds_grant", "{body}");
}

/// The re-scoping door: a token naming its own id and asking for everything.
#[tokio::test]
async fn a_token_cannot_rescope_itself_upwards() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let bearer = bearer_holding(&harness.database, "self", &["api:secrets"]).await;

    let (status, tokens) = get_with_bearer(&harness.router, "/api/v1/api-tokens", &bearer).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    let id = tokens
        .as_array()
        .expect("a list")
        .iter()
        .find(|token| token["label"] == "self")
        .and_then(|token| token["id"].as_str())
        .expect("its own id")
        .to_owned();

    let (status, body) = patch_with_bearer(
        &harness.router,
        &format!("/api/v1/api-tokens/{id}"),
        &bearer,
        serde_json::json!({ "scopes": ["api:*"] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "api.scope_exceeds_grant", "{body}");

    // Unchanged where it counts: still no administration.
    let (status, body) = get_with_bearer(&harness.router, "/api/v1/plugins", &bearer).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

/// A signed-in administrator holds every area and is not limited by the ceiling.
#[tokio::test]
async fn a_session_still_mints_every_area() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = auth_harness(directory.path()).await;
    let session = sign_in(&harness.router, PASSWORD).await;

    let (status, body) = common::post_json_with_cookie(
        &harness.router,
        "/api/v1/api-tokens",
        &session,
        serde_json::json!({ "label": "Everything", "scopes": ["api:*"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        body["token"]["scopes"],
        serde_json::json!(["api:*"]),
        "{body}"
    );
}
