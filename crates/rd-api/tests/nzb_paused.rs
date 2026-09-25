//! RD-107-09: "add paused" must hold for NZBs as well as for links.
//!
//! Two paths reach the Usenet queue and both used to ignore the flag. A hotfolder or uploaded
//! NZB goes through `POST /api/v1/nzb/imports/{id}/enqueue`, which knew nothing but the path
//! id and wrote every download row as `queued`. An NZB *candidate* in a LinkGrabber package
//! short-circuits into the import path inside `collector_enqueue`, which did not carry
//! `start_paused` along — so that one started immediately without so much as a disabled
//! control to warn anybody.

mod common;

use axum::http::StatusCode;
use common::{post_json, test_harness};
use serde_json::json;

/// One-file NZB an indexer would answer with.
const DOCUMENT: &str = r#"<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="tester" subject="release.bin">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="42" number="1">release@example</segment></segments>
  </file>
</nzb>"#;

fn nzb_import(name: &str, digest: &str) -> rd_db::NewNzbImport {
    rd_db::NewNzbImport {
        name: name.to_owned(),
        sha256: digest.repeat(32),
        category_id: None,
        priority: None,
        import_mode: rd_core::ImportMode::Review,
        source: rd_core::IngressSource::Manual,
        source_path: None,
        password: None,
        announce_arrival: false,
        files: vec![rd_db::NewNzbFile {
            subject: format!("{name}.bin"),
            poster: "poster".to_owned(),
            groups: vec!["alt.binaries.test".to_owned()],
            segments: vec![rd_db::NewNzbSegment {
                number: 1,
                bytes: 128,
                message_id: format!("{name}-1@example.test"),
            }],
        }],
    }
}

/// States of every download row of one package, so a paused package cannot hide a queued file.
async fn states(
    harness: &common::Harness,
    package: rd_core::PackageId,
) -> Vec<rd_core::DownloadState> {
    harness
        .database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package)
        .map(|file| file.state)
        .collect()
}

/// The reported defect: a hotfolder NZB waiting in the LinkGrabber, taken over paused.
#[tokio::test]
async fn an_nzb_import_enqueued_paused_reaches_the_queue_paused() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let import = harness
        .database
        .add_nzb_import(nzb_import("paused.nzb", "a1"))
        .await
        .expect("import");

    let (status, package) = post_json(
        &harness.router,
        &format!("/api/v1/nzb/imports/{}/enqueue", import.id),
        json!({ "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{package}");

    let id: rd_core::PackageId = serde_json::from_value(package["id"].clone()).expect("package id");
    assert_eq!(
        states(&harness, id).await,
        vec![rd_core::DownloadState::Paused],
        "the NZB must wait in the download list instead of starting: {package}"
    );
}

/// The control: without the flag — and for every caller that still sends no body at all — the
/// import keeps starting immediately.
#[tokio::test]
async fn an_nzb_import_enqueued_without_the_flag_still_starts() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let import = harness
        .database
        .add_nzb_import(nzb_import("started.nzb", "b2"))
        .await
        .expect("import");

    let (status, package) = post_json(
        &harness.router,
        &format!("/api/v1/nzb/imports/{}/enqueue", import.id),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{package}");

    let id: rd_core::PackageId = serde_json::from_value(package["id"].clone()).expect("package id");
    // Deliberately not `== [Queued]`: the harness runs a real scheduler, so an unpaused row is
    // free to move on at once. What must never happen is that it lands paused.
    let observed = states(&harness, id).await;
    assert!(
        !observed.is_empty() && !observed.contains(&rd_core::DownloadState::Paused),
        "the ordinary enqueue is unchanged: {observed:?} for {package}"
    );
}

/// The silent twin: an NZB *candidate* in a collector package enqueued with `paused`.
///
/// This one never had a disabled button or an error to show for it — the package reported a
/// successful paused enqueue and the download started anyway.
#[tokio::test]
async fn an_nzb_candidate_enqueued_paused_starts_paused() {
    let app = axum::Router::new().route(
        "/release.nzb",
        axum::routing::get(|| async { ([("content-type", "application/x-nzb")], DOCUMENT) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (_, packages, _) = harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some("Release".to_owned()),
            password: None,
            passwords: vec![None],
            category_id: None,
            priority: None,
            providers: vec![Some(rd_core::NZB_PROVIDER.to_owned())],
            file_names: vec![Some("Release".to_owned())],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            urls: vec![
                format!("http://{address}/release.nzb")
                    .parse()
                    .expect("url"),
            ],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let collector_package = packages.first().expect("collector package").id;

    let (status, result) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": [collector_package], "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    assert_eq!(result["failed"], 0, "{result}");

    let id: rd_core::PackageId =
        serde_json::from_value(result["created"][0]["id"].clone()).expect("package id");
    assert_eq!(
        states(&harness, id).await,
        vec![rd_core::DownloadState::Paused],
        "an NZB candidate enqueued paused must not start downloading: {result}"
    );
}
