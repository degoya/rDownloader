//! The bulk action's filter form (RD-1190-15): every file in the named states, or only those of
//! one package, reset without the client listing their ids.

use crate::common;

use axum::http::StatusCode;
use rd_core::DownloadState;

/// Queues one download in a package of its own and walks it to `state`; returns
/// `(download id, package id)`.
async fn download_in(
    harness: &common::Harness,
    name: &str,
    state: DownloadState,
) -> (String, String) {
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        serde_json::json!({
            "url": format!("https://example.invalid/{name}.bin"),
            "package_name": format!("Bulk filter {name}")
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().expect("download id").to_owned();
    let package = created["package_id"]
        .as_str()
        .expect("package id")
        .to_owned();
    let path: &[DownloadState] = match state {
        DownloadState::Failed => &[DownloadState::Resolving, DownloadState::Failed],
        DownloadState::Blocked => &[DownloadState::Blocked],
        DownloadState::Paused => &[DownloadState::Paused],
        other => panic!("no path to {other:?} in this suite"),
    };
    let typed: rd_core::DownloadId = id.parse().expect("download id");
    for &next in path {
        harness
            .database
            .transition_download(typed, next)
            .await
            .expect("transition");
    }
    (id, package)
}

async fn state_of(harness: &common::Harness, id: &str) -> DownloadState {
    harness
        .database
        .get_download(id.parse().expect("download id"))
        .await
        .expect("download")
        .expect("still listed")
        .state
}

#[tokio::test]
async fn a_state_filter_resets_only_the_files_in_those_states() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (failed, _) = download_in(&harness, "state-failed", DownloadState::Failed).await;
    let (blocked, _) = download_in(&harness, "state-blocked", DownloadState::Blocked).await;
    let (paused, _) = download_in(&harness, "state-paused", DownloadState::Paused).await;

    let (status, result) = common::post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        serde_json::json!({ "action": "reset", "filter": { "states": ["failed"] } }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["affected"], 1, "{result}");
    assert_eq!(state_of(&harness, &failed).await, DownloadState::Queued);
    assert_eq!(
        state_of(&harness, &blocked).await,
        DownloadState::Blocked,
        "a state the filter does not name is left alone"
    );
    assert_eq!(state_of(&harness, &paused).await, DownloadState::Paused);

    let (status, result) = common::post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        serde_json::json!({ "action": "reset", "filter": { "states": ["failed", "blocked"] } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(
        result["affected"], 1,
        "only the blocked file was still stuck: {result}"
    );
    assert_eq!(state_of(&harness, &blocked).await, DownloadState::Queued);
    assert_eq!(state_of(&harness, &paused).await, DownloadState::Paused);
}

#[tokio::test]
async fn a_package_filter_stays_inside_its_package() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (inside, package) = download_in(&harness, "package-inside", DownloadState::Failed).await;
    let (outside, _) = download_in(&harness, "package-outside", DownloadState::Failed).await;

    let (status, result) = common::post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        serde_json::json!({
            "action": "reset",
            "filter": { "states": ["failed", "blocked"], "package_id": package }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["affected"], 1, "{result}");
    assert_eq!(state_of(&harness, &inside).await, DownloadState::Queued);
    assert_eq!(
        state_of(&harness, &outside).await,
        DownloadState::Failed,
        "a file of another package is not taken"
    );
}

#[tokio::test]
async fn a_filter_that_selects_nothing_affects_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (paused, _) = download_in(&harness, "nothing-paused", DownloadState::Paused).await;

    let (status, result) = common::post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        serde_json::json!({ "action": "reset", "filter": { "states": ["blocked"] } }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "no match is not an error: {result}");
    assert_eq!(result["affected"], 0);
    assert_eq!(state_of(&harness, &paused).await, DownloadState::Paused);
}

#[tokio::test]
async fn a_filter_is_refused_beside_ids_without_states_and_for_an_unknown_package() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (failed, _) = download_in(&harness, "refused-failed", DownloadState::Failed).await;

    for body in [
        serde_json::json!({ "ids": [failed], "action": "reset", "filter": { "states": ["failed"] } }),
        serde_json::json!({ "action": "reset", "filter": { "states": [] } }),
    ] {
        let (status, result) =
            common::post_json(&harness.router, "/api/v1/downloads/bulk", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{result}");
        assert_eq!(result["code"], "request.bulk_filter");
    }

    let unknown = rd_core::PackageId::new().to_string();
    let (status, result) = common::post_json(
        &harness.router,
        "/api/v1/downloads/bulk",
        serde_json::json!({
            "action": "reset",
            "filter": { "states": ["failed"], "package_id": unknown }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{result}");
    assert_eq!(result["code"], "package.not_found");
    assert_eq!(
        state_of(&harness, &failed).await,
        DownloadState::Failed,
        "a refused request changes nothing"
    );
}
