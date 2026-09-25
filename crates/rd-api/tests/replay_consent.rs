//! Integration tests for the replay consent gate: a captured POST cannot be enqueued until
//! a person has approved exactly that request, and nothing about it ever leaks over REST.

mod common;

use axum::http::StatusCode;
use base64::{Engine as _, engine::general_purpose::STANDARD};

/// A form value and a URL signature that must never appear in any response.
const BODY_SECRET: &str = "s3cr3t-form-value";
const URL_SIGNATURE: &str = "abcdef0123456789deadbeef";

/// A captured POST download as the v2 extension sends it.
fn post_capture_payload() -> serde_json::Value {
    let body = format!("id=42&token={BODY_SECRET}&name=movie.mkv");
    serde_json::json!({
        "source": "browser_download",
        "source_label": "Chrome",
        "links": [{
            "url": "https://hoster.example/dl/42",
            "file_name": "movie.mkv",
            "request": {
                "effective_url": format!(
                    "https://cdn.example.net/f.bin?X-Amz-Signature={URL_SIGNATURE}\
                     &X-Amz-Date=20990101T000000Z&X-Amz-Expires=900"
                ),
                "method": "POST",
                "referrer": "https://hoster.example/page",
                "user_agent": "Mozilla/5.0",
                "headers": [
                    { "name": "content-type", "value": "application/x-www-form-urlencoded" },
                    { "name": "accept", "value": "*/*" }
                ],
                "body_b64": STANDARD.encode(body.as_bytes())
            }
        }]
    })
}

#[tokio::test]
async fn a_captured_post_is_stored_with_field_names_but_never_its_values() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;

    let (status, payload) = common::post_capture(&router, post_capture_payload()).await;
    assert_eq!(status, StatusCode::CREATED, "{payload}");

    let request = &payload["candidates"][0]["request"];
    assert_eq!(request["method"], "POST");
    assert_eq!(request["replayable"], true);
    // Field names are what makes the consent dialog meaningful; values are not.
    assert_eq!(
        request["body"]["field_names"],
        serde_json::json!(["id", "token", "name"])
    );
    assert_eq!(request["body"]["stored"], true);
    assert_eq!(
        request["body"]["content_type"],
        "application/x-www-form-urlencoded"
    );
    // The origin set is server-derived from the link and its redirect target.
    assert_eq!(
        request["approved_origins"],
        serde_json::json!(["https://hoster.example", "https://cdn.example.net"])
    );
    assert!(
        request["expires_at"].is_string(),
        "expiry read from the URL"
    );

    let serialized = payload.to_string();
    assert!(!serialized.contains(BODY_SECRET), "body value leaked");
    assert!(!serialized.contains("body_b64"), "raw body echoed back");

    // Reading the candidates back must not leak either.
    let (status, listed) = common::get_json(&router, "/api/v1/collector/candidates").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!listed.to_string().contains(BODY_SECRET), "{listed}");
}

#[tokio::test]
async fn the_preview_names_every_credential_category_and_redacts_the_signature() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (_, payload) = common::post_capture(&router, post_capture_payload()).await;
    let candidate_id = payload["candidates"][0]["id"].as_str().expect("id");

    let (status, preview) = common::get_json(
        &router,
        &format!("/api/v1/collector/candidates/{candidate_id}/replay-preview"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");

    assert_eq!(preview["method"], "POST");
    assert_eq!(preview["target_origin"], "https://cdn.example.net");
    // A signed URL and a form body are exactly the two categories this capture sends.
    assert_eq!(
        preview["credential_categories"],
        serde_json::json!(["signed_query", "form_fields"])
    );
    assert!(
        preview["template_hash"]
            .as_str()
            .is_some_and(|hash| !hash.is_empty())
    );
    assert!(preview["consent"].is_null(), "nothing approved yet");

    let serialized = preview.to_string();
    assert!(
        !serialized.contains(URL_SIGNATURE),
        "signature leaked: {serialized}"
    );
    assert!(!serialized.contains(BODY_SECRET), "body value leaked");
    // The parameter name survives so the URL stays readable in support.
    assert!(serialized.contains("X-Amz-Signature"));
}

#[tokio::test]
async fn enqueueing_without_consent_is_refused_and_succeeds_after_approval() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (_, payload) = common::post_capture(&router, post_capture_payload()).await;
    let candidate_id = payload["candidates"][0]["id"].as_str().expect("id");
    let package_id = payload["packages"][0]["id"].as_str().expect("package");

    common::wait_for_candidates_ready(&router).await;
    let enqueue = format!("/api/v1/collector/packages/{package_id}/enqueue");
    let (status, refused) = common::post_json(&router, &enqueue, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "replay.consent_required");

    // A hash that does not match the current template is a different decision.
    let consent = format!("/api/v1/collector/candidates/{candidate_id}/replay-consent");
    let (status, stale) = common::post_json(
        &router,
        &consent,
        serde_json::json!({ "template_hash": "not-the-right-hash" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert_eq!(stale["code"], "replay.template_changed");

    let (_, preview) = common::get_json(
        &router,
        &format!("/api/v1/collector/candidates/{candidate_id}/replay-preview"),
    )
    .await;
    let hash = preview["template_hash"].as_str().expect("hash");

    let (status, granted) = common::post_json(
        &router,
        &consent,
        serde_json::json!({ "template_hash": hash }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{granted}");
    assert_eq!(granted["consent"]["template_hash"], hash);

    let (status, enqueued) = common::post_json(&router, &enqueue, serde_json::json!({})).await;
    assert_eq!(status, StatusCode::CREATED, "{enqueued}");
    assert!(!enqueued.to_string().contains(BODY_SECRET));
}

#[tokio::test]
async fn consent_cannot_widen_the_origins_the_server_derived() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (_, payload) = common::post_capture(&router, post_capture_payload()).await;
    let candidate_id = payload["candidates"][0]["id"].as_str().expect("id");
    let (_, preview) = common::get_json(
        &router,
        &format!("/api/v1/collector/candidates/{candidate_id}/replay-preview"),
    )
    .await;
    let hash = preview["template_hash"].as_str().expect("hash");

    let (status, refused) = common::post_json(
        &router,
        &format!("/api/v1/collector/candidates/{candidate_id}/replay-consent"),
        serde_json::json!({
            "template_hash": hash,
            "approved_origins": ["https://evil.example"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "replay.origin_not_approved");

    // Narrowing the set is always allowed.
    let (status, narrowed) = common::post_json(
        &router,
        &format!("/api/v1/collector/candidates/{candidate_id}/replay-consent"),
        serde_json::json!({
            "template_hash": hash,
            "approved_origins": ["https://cdn.example.net"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{narrowed}");
    assert_eq!(
        narrowed["consent"]["approved_origins"],
        serde_json::json!(["https://cdn.example.net"])
    );
}

#[tokio::test]
async fn a_capture_that_cannot_be_reproduced_is_kept_and_explained() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;

    let mut payload = post_capture_payload();
    payload["links"][0]["request"]["has_file_upload"] = serde_json::json!(true);
    let (status, response) = common::post_capture(&router, payload).await;
    // Accepted, not rejected: the browser download is already gone by now, so refusing
    // would leave the user with nothing at all.
    assert_eq!(status, StatusCode::CREATED, "{response}");

    let request = &response["candidates"][0]["request"];
    assert_eq!(request["replayable"], false);
    assert_eq!(request["blocked_reason"], "file_upload");
    // A refused body is never stored, but its metadata still explains the block.
    assert_eq!(request["body"]["stored"], false);
    assert!(!response.to_string().contains(BODY_SECRET));

    let candidate_id = response["candidates"][0]["id"].as_str().expect("id");
    let (status, refused) = common::post_json(
        &router,
        &format!("/api/v1/collector/candidates/{candidate_id}/replay-consent"),
        serde_json::json!({ "template_hash": "any" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["code"], "replay.not_replayable");
    assert_eq!(refused["params"]["reason"], "file_upload");
}

#[tokio::test]
async fn a_plain_captured_get_still_needs_no_approval() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = common::test_router(directory.path()).await;

    // Browser interception without credentials must behave exactly as it did before replay.
    let (status, payload) = common::post_capture(
        &router,
        serde_json::json!({
            "source": "browser_download",
            "links": [{
                "url": "https://files.example.com/report.pdf",
                "request": { "method": "GET", "headers": [] }
            }]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{payload}");
    let package_id = payload["packages"][0]["id"].as_str().expect("package");

    common::wait_for_candidates_ready(&router).await;
    let (status, enqueued) = common::post_json(
        &router,
        &format!("/api/v1/collector/packages/{package_id}/enqueue"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{enqueued}");
}
