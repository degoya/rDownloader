//! "Clear the entire list" (RD-180-21): every package goes, running ones included.
//!
//! The selection rule is unit-tested in `package_clear`; what is checked here is the path a
//! running file takes through the scheduler — cancelled, then removed — what stays on disk, and
//! that the partial data goes only when it was asked for. The scheduler is parked and the
//! running file's lifecycle written by hand, as the other queue suites do: a live supervisor
//! would race the test for the row.

use crate::common;

use axum::http::StatusCode;
use rd_core::{DownloadId, DownloadState};
use serde_json::json;

/// One package with one direct download; answers `(download id, package destination)`.
async fn queued(
    harness: &common::Harness,
    package: &str,
    file: &str,
) -> (DownloadId, std::path::PathBuf) {
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": format!("https://example.invalid/{file}"), "package_name": package }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (_, packages) = common::get_json(&harness.router, "/api/v1/packages").await;
    let destination = packages
        .as_array()
        .expect("packages")
        .iter()
        .find(|row| row["id"] == created["package_id"])
        .and_then(|row| row["destination"].as_str())
        .expect("the package has a folder")
        .into();
    (
        created["id"]
            .as_str()
            .expect("id")
            .parse()
            .expect("download id"),
        destination,
    )
}

async fn walk(harness: &common::Harness, id: DownloadId, states: &[DownloadState]) {
    for state in states {
        harness
            .database
            .transition_download(id, *state)
            .await
            .expect("transition");
    }
}

/// A running file with a staging file and a tool fragment beside its target, and a finished
/// one with its payload. Answers the running id and the three paths.
async fn running_and_finished(
    harness: &common::Harness,
) -> (
    DownloadId,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let (running, running_folder) = queued(harness, "Running", "movie.mkv").await;
    walk(
        harness,
        running,
        &[DownloadState::Resolving, DownloadState::Downloading],
    )
    .await;
    let staging = running_folder.join(".rdownloader");
    std::fs::create_dir_all(&staging).expect("staging");
    let part = staging.join(format!("{running}.part"));
    std::fs::write(&part, b"partial").expect("part file");
    let fragment = running_folder.join("movie.f137.mkv.part");
    std::fs::write(&fragment, b"partial").expect("fragment");

    let (finished, finished_folder) = queued(harness, "Finished", "done.bin").await;
    walk(
        harness,
        finished,
        &[
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
            DownloadState::Completed,
        ],
    )
    .await;
    // Its post-processing settled, as `auto_remove` does it: with the scheduler parked nothing
    // runs the pipeline, and a package waiting for it is the one the clear rightly spares.
    let package_id = harness
        .database
        .get_download(finished)
        .await
        .expect("read")
        .expect("the finished file")
        .package_id;
    harness
        .database
        .set_package_state(
            package_id,
            rd_core::PackageState::Completed,
            None,
            None,
            None,
        )
        .await
        .expect("package completed");
    // The completion queued the package for post-processing; there is nothing to unpack, so the
    // pipeline lets go of it at once — the clear spares it until then.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while harness
        .state
        .extraction
        .pending()
        .await
        .contains(&package_id)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "post-processing never let go of the finished package"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    std::fs::create_dir_all(&finished_folder).expect("finished folder");
    let payload = finished_folder.join("done.bin");
    std::fs::write(&payload, b"payload").expect("payload");
    (running, part, fragment, payload)
}

async fn clear(
    harness: &common::Harness,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    common::post_json(&harness.router, "/api/v1/packages/clear", body).await
}

#[tokio::test]
async fn the_entire_list_is_not_cleared_without_a_confirmation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (running, ..) = running_and_finished(&harness).await;

    let (status, body) = clear(&harness, json!({ "scope": "everything" })).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "package.clear_unconfirmed");
    let row = harness
        .database
        .get_download(running)
        .await
        .expect("read")
        .expect("still queued");
    assert_eq!(row.state, DownloadState::Downloading, "nothing was stopped");
}

#[tokio::test]
async fn clearing_the_entire_list_cancels_the_running_file_and_keeps_finished_data() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (running, part, fragment, payload) = running_and_finished(&harness).await;

    let (status, body) = clear(
        &harness,
        json!({ "scope": "everything", "confirmed": true }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 2, "{body}");
    assert_eq!(body["skipped"], json!([]));
    let (_, packages) = common::get_json(&harness.router, "/api/v1/packages").await;
    assert_eq!(packages, json!([]), "the list is empty");
    assert!(
        harness
            .database
            .get_download(running)
            .await
            .expect("read")
            .is_none(),
        "the running file went through cancel and removal"
    );
    assert!(payload.exists(), "a finished file stays in its folder");
    assert!(!part.exists(), "the staging file goes with every removal");
    assert!(
        fragment.exists(),
        "partial data beside the target stays unless asked"
    );

    let records = harness
        .database
        .query_audit_records(&rd_db::AuditQuery {
            limit: 50,
            ..rd_db::AuditQuery::default()
        })
        .await
        .expect("audit");
    let deleted = records
        .into_iter()
        .map(|record| serde_json::to_value(record).expect("json"))
        .filter(|record| record["action"] == "package_deleted")
        .count();
    assert_eq!(deleted, 2, "one audit record per package");
}

#[tokio::test]
async fn partial_data_goes_only_when_it_was_asked_for() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (_, part, fragment, payload) = running_and_finished(&harness).await;

    let (status, body) = clear(
        &harness,
        json!({ "scope": "everything", "confirmed": true, "delete_partial": true }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 2, "{body}");
    assert!(
        !fragment.exists(),
        "the unfinished file's fragment is deleted"
    );
    assert!(!part.exists());
    assert!(payload.exists(), "a finished file is never partial data");
}

/// The other scopes are unchanged: a running package is still refused by `all`.
#[tokio::test]
async fn removing_all_stopped_packages_still_spares_the_running_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (running, part, ..) = running_and_finished(&harness).await;

    let (status, body) = clear(&harness, json!({ "scope": "all" })).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed"], 1, "{body}");
    assert_eq!(body["skipped"][0]["code"], "package.members_active");
    assert!(
        harness
            .database
            .get_download(running)
            .await
            .expect("read")
            .is_some()
    );
    assert!(part.exists(), "its data is untouched");
}
