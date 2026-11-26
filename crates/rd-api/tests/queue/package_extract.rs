//! Post-processing a package on request, plain and forced (audit 2026-11-25, TST-7): queued for a
//! package with a finished file, refused for one without. What the pipeline then does is
//! `rd-extract`'s own suite.

use crate::common;

use axum::http::StatusCode;
use common::{parked_harness, post_json};
use rd_core::{DownloadState, PackageId};
use serde_json::json;

/// A package of one file, run through the states up to `Completed` when `finished`.
async fn package(harness: &common::Harness, root: &std::path::Path, finished: bool) -> PackageId {
    let name = if finished { "Finished" } else { "Unfinished" };
    let download = crate::download_list::package_of(harness, root, name, 1).await[0];
    if finished {
        for state in [
            DownloadState::Queued,
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
            DownloadState::Completed,
        ] {
            harness
                .database
                .transition_download(download, state)
                .await
                .expect("transition");
        }
    }
    harness
        .database
        .get_download(download)
        .await
        .expect("read")
        .expect("the row")
        .package_id
}

#[tokio::test]
async fn both_routes_queue_a_package_with_a_finished_file_and_no_other() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = parked_harness(directory.path()).await;
    let finished = package(&harness, directory.path(), true).await;
    let unfinished = package(&harness, directory.path(), false).await;

    let routes = |id: PackageId| {
        [
            format!("/api/v1/packages/{id}/extract"),
            format!("/api/v1/packages/{id}/extract/force"),
        ]
    };
    for uri in routes(finished) {
        let (status, body) = post_json(&harness.router, &uri, json!({})).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{uri}: {body}");
        assert_eq!(body["code"], "package.extract_queued", "{uri}: {body}");
    }
    for uri in routes(unfinished)
        .into_iter()
        .chain(routes(PackageId::new()))
    {
        let (status, body) = post_json(&harness.router, &uri, json!({})).await;
        assert_eq!(status, StatusCode::CONFLICT, "{uri}: {body}");
        assert_eq!(body["code"], "package.nothing_to_extract", "{uri}: {body}");
    }
}
