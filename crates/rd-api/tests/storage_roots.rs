//! Storage roots over HTTP: what the form is told about a root it just saved.
//!
//! Two promises. Every root carries whether its path outlives the container, so the routing
//! view can flag one that was configured into the container's writable layer -- including
//! roots created long before the check existed. And the default flag is answered by the
//! server, not guessed by the form: the first root is the default whatever was submitted, and
//! the last one cannot give the flag up.

mod common;

use axum::http::StatusCode;
use common::{delete_json, get_json, post_json, put_json, test_router};
use serde_json::json;

fn body(name: &str, path: &std::path::Path, is_default: bool) -> serde_json::Value {
    json!({
        "name": name,
        "path": path.to_string_lossy(),
        "is_default": is_default,
        "minimum_free_bytes": null,
    })
}

#[tokio::test]
async fn every_root_reports_whether_its_path_is_persistent() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let router = test_router(temporary.path()).await;

    let (status, created) = post_json(
        &router,
        "/api/v1/storage-roots",
        body("Downloads", &temporary.path().join("downloads"), false),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert!(
        created["persistence"].is_string(),
        "the create response must already carry the verdict: {created}"
    );

    let (status, listed) = get_json(&router, "/api/v1/storage-roots").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        listed[0]["persistence"].is_string(),
        "an existing root must be classified too, not only a fresh one: {listed}"
    );
    // Which verdict it is depends on the machine the suite runs on -- a tmpfs /tmp is
    // genuinely ephemeral. The verdicts themselves are pinned against fixed mount tables in
    // rd-files; here only the plumbing is under test.
    assert!(
        ["persistent", "ephemeral", "unknown"]
            .contains(&listed[0]["persistence"].as_str().expect("string")),
        "unexpected verdict: {listed}"
    );
    assert_eq!(
        created["persistence"], listed[0]["persistence"],
        "create and list must agree about the same root"
    );
}

#[tokio::test]
async fn the_first_root_is_the_default_even_when_the_form_did_not_ask() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let router = test_router(temporary.path()).await;

    let (_, created) = post_json(
        &router,
        "/api/v1/storage-roots",
        body("Downloads", &temporary.path().join("downloads"), false),
    )
    .await;

    assert_eq!(
        created["is_default"], true,
        "the response has to say so, or the form shows a root with no default badge"
    );
}

#[tokio::test]
async fn the_last_default_cannot_be_given_up() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let router = test_router(temporary.path()).await;
    let downloads = temporary.path().join("downloads");
    let (_, created) = post_json(
        &router,
        "/api/v1/storage-roots",
        body("Downloads", &downloads, false),
    )
    .await;
    let id = created["id"].as_str().expect("id").to_owned();

    let (status, updated) = put_json(
        &router,
        &format!("/api/v1/storage-roots/{id}"),
        body("Downloads", &downloads, false),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        updated["is_default"], true,
        "coerced back, and the response says so instead of failing the edit"
    );
}

#[tokio::test]
async fn deleting_the_default_hands_the_flag_to_another_root() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let router = test_router(temporary.path()).await;
    let (_, first) = post_json(
        &router,
        "/api/v1/storage-roots",
        body("Bravo", &temporary.path().join("bravo"), false),
    )
    .await;
    post_json(
        &router,
        "/api/v1/storage-roots",
        body("Alpha", &temporary.path().join("alpha"), false),
    )
    .await;
    let id = first["id"].as_str().expect("id").to_owned();

    let (status, _) = delete_json(&router, &format!("/api/v1/storage-roots/{id}")).await;
    assert_eq!(status, StatusCode::OK);

    let (_, listed) = get_json(&router, "/api/v1/storage-roots").await;
    let defaults: Vec<&str> = listed
        .as_array()
        .expect("array")
        .iter()
        .filter(|root| root["is_default"] == true)
        .map(|root| root["name"].as_str().expect("name"))
        .collect();
    assert_eq!(defaults, vec!["Alpha"]);
}

#[tokio::test]
async fn the_setup_status_counts_roots_that_will_not_survive() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let router = test_router(temporary.path()).await;
    post_json(
        &router,
        "/api/v1/storage-roots",
        body("Downloads", &temporary.path().join("downloads"), false),
    )
    .await;

    let (status, setup) = get_json(&router, "/api/v1/setup/status").await;
    let (_, listed) = get_json(&router, "/api/v1/storage-roots").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(setup["storage_roots"], 1);
    let ephemeral = listed
        .as_array()
        .expect("array")
        .iter()
        .filter(|root| root["persistence"] == "ephemeral")
        .count();
    assert_eq!(
        setup["ephemeral_storage_roots"], ephemeral,
        "the readiness card and the routing view must not disagree about the same roots"
    );
}
