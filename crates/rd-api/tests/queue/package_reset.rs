//! "Reset the package completely" (RD-1390-07): `POST /api/v1/packages/{id}/reset`.
//!
//! What goes and what stays on disk is tested in the scheduler; here the route's answers - the
//! count, the setting that keeps the unpacked results, and the stable codes of each refusal. The
//! scheduler is parked, as in the other queue suites, so a reset row stays queued.

use crate::common;

use axum::http::StatusCode;
use rd_core::{DownloadId, DownloadState, PackageId, PackageState};
use serde_json::json;

/// One package with one direct download; answers `(download, package, folder)`.
async fn queued(
    harness: &common::Harness,
    package: &str,
) -> (DownloadId, PackageId, std::path::PathBuf) {
    let (status, created) = common::post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": "https://example.invalid/set.part1.rar", "package_name": package }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id: DownloadId = created["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("download id");
    let package = harness
        .database
        .get_download(id)
        .await
        .expect("read")
        .expect("the download")
        .package_id;
    let folder = harness
        .database
        .get_package(package)
        .await
        .expect("read")
        .expect("the package")
        .destination
        .into();
    (id, package, folder)
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

/// A package whose only volume finished and whose unpack failed, after putting `film.mkv`
/// beside it; `theirs.txt` was in the folder before. Answers the package and its folder.
async fn failed_package(harness: &common::Harness) -> (PackageId, std::path::PathBuf) {
    let (volume, package, folder) = queued(harness, "Corrupt").await;
    walk(
        harness,
        volume,
        &[
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
        ],
    )
    .await;
    // Settled before the file completes, so the completion listener asks the pipeline for
    // nothing (the reason `clear_list` gives).
    harness
        .database
        .set_package_state(package, PackageState::Failed, None, None, None)
        .await
        .expect("package failed");
    walk(harness, volume, &[DownloadState::Completed]).await;
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(folder.join("set.part1.rar"), b"volume").expect("payload");
    std::fs::write(folder.join("theirs.txt"), b"someone else's").expect("foreign file");
    std::fs::write(folder.join("film.mkv"), b"half a film").expect("unpacked file");
    harness
        .database
        .record_package_outputs(
            package,
            vec![folder.join("film.mkv").to_string_lossy().into_owned()],
        )
        .await
        .expect("outputs");
    (package, folder)
}

async fn reset(harness: &common::Harness, package: PackageId) -> (StatusCode, serde_json::Value) {
    common::post_json(
        &harness.router,
        &format!("/api/v1/packages/{package}/reset"),
        json!({}),
    )
    .await
}

#[tokio::test]
async fn a_failed_package_is_reset_whole_and_a_strangers_file_stays() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (package, folder) = failed_package(&harness).await;

    let (status, body) = reset(&harness, package).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["code"], "package.reset", "{body}");
    assert_eq!(body["params"]["count"], "1", "{body}");
    assert!(
        !folder.join("set.part1.rar").exists(),
        "the finished volume"
    );
    assert!(!folder.join("film.mkv").exists(), "what the unpack created");
    assert!(
        folder.join("theirs.txt").exists(),
        "a stranger's file stays"
    );
    let files = harness
        .database
        .downloads_for_package(package)
        .await
        .expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].state, DownloadState::Queued);
    let row = harness
        .database
        .get_package(package)
        .await
        .expect("read")
        .expect("the package stays in the queue");
    assert_eq!(row.state, PackageState::Queued);
    assert_eq!(row.name, "Corrupt");
}

/// With `package_reset_delete_extracted` off the unpacked results stay; the download goes.
#[tokio::test]
async fn the_setting_keeps_the_unpacked_results() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (package, folder) = failed_package(&harness).await;
    let (_, mut settings) = common::get_json(&harness.router, "/api/v1/settings").await;
    assert_eq!(
        settings["package_reset_delete_extracted"], true,
        "on by default"
    );
    settings["package_reset_delete_extracted"] = json!(false);
    // Writing the document re-applies its login switch; the harness set only the flag.
    settings["admin_login_disabled"] = json!(true);
    let (status, body) = common::put_json(&harness.router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = reset(&harness, package).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!folder.join("set.part1.rar").exists(), "the download goes");
    assert!(
        folder.join("film.mkv").exists(),
        "the unpacked result stays"
    );
}

#[tokio::test]
async fn a_package_without_a_failure_is_refused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (_, package, _) = queued(&harness, "Fine").await;

    let (status, body) = reset(&harness, package).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "package.reset_not_failed", "{body}");
}

#[tokio::test]
async fn a_running_file_refuses_the_reset_and_deletes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;
    let (package, folder) = failed_package(&harness).await;
    // A second volume of the same package, under way.
    let running = harness
        .database
        .create_download(rd_db::NewDownload {
            id: DownloadId::new(),
            package_id: package,
            source: url::Url::parse("https://example.invalid/set.part2.rar").expect("url"),
            file_name: "set.part2.rar".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            secret_fragment: None,
            mirror_group: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("second volume")
        .id;
    walk(
        &harness,
        running,
        &[DownloadState::Resolving, DownloadState::Downloading],
    )
    .await;

    let (status, body) = reset(&harness, package).await;

    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "package.reset_running", "{body}");
    assert!(folder.join("set.part1.rar").exists(), "nothing was deleted");
}

#[tokio::test]
async fn an_unknown_package_is_not_found() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::parked_harness(directory.path()).await;

    let (status, body) = reset(&harness, PackageId::new()).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "package.not_found", "{body}");
}
