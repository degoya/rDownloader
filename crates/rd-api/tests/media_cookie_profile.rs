//! RD-080-04: choosing the cookie profile a link is queued with.
//!
//! The materialisation itself is covered in `rd-media`; what is tested here is the choice
//! being *reviewable* — a profile can be pinned before the link is queued, a profile that
//! does not cover the link is refused while the user is still looking at it, and the choice
//! survives into the queue row instead of being re-derived there.

mod common;

use common::{get_json, post_json, put_json, test_router, wait_for_candidates_ready};
use serde_json::json;

/// Creates an enabled cookie profile for `host` and returns its id.
async fn create_profile(router: &axum::Router, host: &str) -> String {
    let (status, profile) = post_json(
        router,
        "/api/v1/auth-profiles",
        json!({
            "name": format!("{host} session"),
            "scope": host,
            "include_subdomains": true,
            "method": "cookies",
            "username": null,
            "secret": format!(".{host}\tTRUE\t/\tTRUE\t2000000000\tSID\tabc"),
            "certificate_pem": null,
            "expires_at": null,
            "enabled": true,
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{profile}");
    profile["id"].as_str().expect("profile id").to_owned()
}

/// Adds one link and returns its candidate id.
async fn intake(router: &axum::Router, url: &str) -> String {
    let (status, _) = post_json(
        router,
        "/api/v1/collector/batches",
        json!({ "text": url, "source": "api", "source_label": null, "package_name": null, "password": null }),
    )
    .await;
    assert!(status.is_success(), "intake failed: {status}");
    wait_for_candidates_ready(router).await;
    let (_, candidates) = get_json(router, "/api/v1/collector/candidates").await;
    candidates[0]["id"]
        .as_str()
        .expect("candidate id")
        .to_owned()
}

#[tokio::test]
async fn a_profile_can_be_pinned_to_a_link_before_it_is_queued() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let profile = create_profile(&router, "example.test").await;
    let candidate = intake(&router, "https://example.test/watch?v=1").await;

    let (status, updated) = put_json(
        &router,
        &format!("/api/v1/collector/candidates/{candidate}/auth-profile"),
        json!({ "mode": "pinned", "profile_id": profile }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{updated}");
    assert_eq!(updated["auth_profile"]["mode"], "pinned");
    assert_eq!(updated["auth_profile"]["id"], profile.as_str());
}

#[tokio::test]
async fn a_profile_for_another_site_is_refused_while_the_link_is_still_in_review() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let profile = create_profile(&router, "example.test").await;
    let candidate = intake(&router, "https://other.test/watch?v=1").await;

    let (status, body) = put_json(
        &router,
        &format!("/api/v1/collector/candidates/{candidate}/auth-profile"),
        json!({ "mode": "pinned", "profile_id": profile }),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
    assert_eq!(body["code"], "collector.auth_profile_scope_mismatch");
}

#[tokio::test]
async fn pinning_without_a_profile_id_is_a_bad_request() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    let candidate = intake(&router, "https://example.test/watch?v=1").await;

    let (status, body) = put_json(
        &router,
        &format!("/api/v1/collector/candidates/{candidate}/auth-profile"),
        json!({ "mode": "pinned" }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "collector.auth_profile_missing");
}

#[tokio::test]
async fn sending_nothing_is_a_distinct_choice_from_letting_the_scope_decide() {
    let temp = tempfile::tempdir().expect("tempdir");
    let router = test_router(temp.path()).await;
    // A matching profile exists, so "none" has to be stored rather than collapsing to the
    // automatic choice — that difference is the whole reason the field has three states.
    create_profile(&router, "example.test").await;
    let candidate = intake(&router, "https://example.test/watch?v=1").await;
    let uri = format!("/api/v1/collector/candidates/{candidate}/auth-profile");

    let (status, updated) = put_json(&router, &uri, json!({ "mode": "none" })).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{updated}");
    assert_eq!(updated["auth_profile"]["mode"], "none");

    let (status, updated) = put_json(&router, &uri, json!({ "mode": "auto" })).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{updated}");
    assert_eq!(updated["auth_profile"]["mode"], "auto");
}

#[tokio::test]
async fn the_choice_survives_a_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let profile;
    let candidate;
    {
        let router = test_router(temp.path()).await;
        profile = create_profile(&router, "example.test").await;
        candidate = intake(&router, "https://example.test/watch?v=1").await;
        let (status, _) = put_json(
            &router,
            &format!("/api/v1/collector/candidates/{candidate}/auth-profile"),
            json!({ "mode": "pinned", "profile_id": profile }),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::OK);
    }

    let router = test_router(temp.path()).await;
    let (_, candidates) = get_json(&router, "/api/v1/collector/candidates").await;
    let stored = candidates
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == candidate.as_str())
        .expect("candidate survived");
    assert_eq!(stored["auth_profile"]["mode"], "pinned");
    assert_eq!(stored["auth_profile"]["id"], profile.as_str());
}
