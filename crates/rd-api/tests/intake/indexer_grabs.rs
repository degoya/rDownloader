//! RD-1490-02: a link behind an indexer proxy that counts grabs is fetched once.
//!
//! Prowlarr refuses the probe's `HEAD` with `405`, ignores the one-byte range of the GET that
//! follows and sends the whole file — and counts every such answer as a grab. Its download link
//! in the LinkGrabber used to be asked three times: the direct probe, the reclassification's
//! own probe and the read of the torrent. The server here behaves like that and counts every
//! GET, ranged or not: the check and the enqueue together make exactly one.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Router,
    http::{Method, StatusCode, header},
    response::IntoResponse,
};
use serde_json::json;

use crate::common;
use crate::torrent::{add_rerouted, enqueue_source, fixture_hash, multi_file_torrent};

/// One-file NZB an indexer would answer with.
const NZB: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
  <file poster="tester" subject="release.bin">
    <groups><group>alt.binaries.test</group></groups>
    <segments><segment bytes="42" number="1">release@example</segment></segments>
  </file>
</nzb>"#;

/// An indexer proxy's download endpoint: `405` to `HEAD`, the whole document to every GET
/// whatever its range, with the headers Newznab establishes. Counts the GETs.
async fn indexer(
    content_type: &'static str,
    document: Vec<u8>,
) -> (rd_testkit::TestServer, String, Arc<AtomicUsize>) {
    let grabs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&grabs);
    let app = Router::new().fallback(move |method: Method| {
        let counter = Arc::clone(&counter);
        let document = document.clone();
        async move {
            if method != Method::GET {
                return StatusCode::METHOD_NOT_ALLOWED.into_response();
            }
            counter.fetch_add(1, Ordering::SeqCst);
            (
                [
                    (header::CONTENT_TYPE.as_str(), content_type),
                    ("x-dnzb-name", "Indexer.Release"),
                ],
                document,
            )
                .into_response()
        }
    });
    let server = rd_testkit::serve(app).await;
    let link = format!(
        "http://{}/1/download?apikey=secret&link=token",
        server.addr()
    );
    (server, link, grabs)
}

async fn a_torrent_behind_the_proxy_is_grabbed_once(content_type: &'static str) {
    let directory = tempfile::tempdir_in(".").expect("tempdir");
    let router = common::test_router(directory.path()).await;
    let (_server, link, grabs) = indexer(content_type, multi_file_torrent()).await;

    let packages = add_rerouted(&router, &link).await;
    assert_eq!(packages[0]["name"], "release", "{packages}");
    let source = enqueue_source(&router, &packages).await;

    assert_eq!(grabs.load(Ordering::SeqCst), 1, "{content_type}");
    assert_eq!(source.scheme(), "file", "{source}");
    let stored = source.to_file_path().expect("stored torrent path");
    assert_eq!(
        stored.file_name().and_then(|name| name.to_str()),
        Some(format!("{}.torrent", fixture_hash()).as_str())
    );
}

#[tokio::test]
async fn a_torrent_the_proxy_names_is_grabbed_once() {
    a_torrent_behind_the_proxy_is_grabbed_once("application/x-bittorrent").await;
}

/// Recognised by its first bytes in the probe's own answer: no sniff is sent after it.
#[tokio::test]
async fn a_torrent_the_proxy_does_not_name_is_grabbed_once() {
    a_torrent_behind_the_proxy_is_grabbed_once("application/octet-stream").await;
}

/// The NZB the check read whole is kept for the enqueue, with the headers it came with: the
/// queue names it after `X-DNZB-Name` without asking the indexer again.
#[tokio::test]
async fn an_nzb_behind_the_proxy_is_grabbed_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;
    let (_server, link, grabs) = indexer("application/x-nzb", NZB.as_bytes().to_vec()).await;

    let (status, body) = common::post_json(
        router,
        "/api/v1/collector/batches",
        json!({
            "text": link,
            // The person's own paste: the fixture serves from this machine (RD-1190-18).
            "source": "manual",
            "source_label": null,
            "package_name": null,
            "password": null
        }),
    )
    .await;
    assert!(status.is_success(), "intake failed: {body}");
    let package = common::eventually_ok(
        common::WAIT,
        "the check never routed the link to the NZB import",
        || async move {
            let (_, candidates) = common::get_json(router, "/api/v1/collector/candidates").await;
            let candidate = &candidates[0];
            if candidate["provider"] == "nzb" && candidate["state"] == "online" {
                Ok(candidate["package_id"].clone())
            } else {
                Err(candidates)
            }
        },
    )
    .await;
    assert_eq!(
        grabs.load(Ordering::SeqCst),
        1,
        "the check reads the NZB once"
    );

    let (status, result) = common::post_json(
        router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": [package] }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{result}");
    assert_eq!(result["failed"], 0, "{result}");
    assert_eq!(
        grabs.load(Ordering::SeqCst),
        1,
        "the enqueue takes the kept NZB"
    );
    let (_, packages) = common::get_json(router, "/api/v1/packages").await;
    assert!(
        packages
            .as_array()
            .expect("packages")
            .iter()
            .any(|entry| entry["name"] == "Indexer.Release"),
        "the kept headers name the release: {packages}"
    );
}
