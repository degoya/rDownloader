//! A Metalink file's mirrors, from the LinkGrabber into the queue (RD-150-03).
//!
//! The parser and the transfer are tested where they live. What is checked here is the path
//! between them that only shows end to end: a candidate carrying a source set becomes one
//! download whose sources are every mirror in the set's order, whose expected checksum is the
//! set's whole-file hash, and whose sources the REST route reports — redacted — with the piece
//! hashes summarised. And the online check in front of it: a mirror a relayed Metalink names at
//! this machine is marked and never requested.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::http::StatusCode;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use crate::common::{eventually, get_json, post_json, test_harness};
use serde_json::json;

fn metalink_set() -> rd_core::SourceSet {
    rd_core::SourceSet::checked(
        [
            (
                "https://second.example/image.iso?token=abcdef0123456789".to_owned(),
                Some(2),
                Some("fr".to_owned()),
            ),
            (
                "https://first.example/image.iso".to_owned(),
                Some(1),
                Some("de".to_owned()),
            ),
            ("ftp://third.example/image.iso".to_owned(), None, None),
        ],
        Some(32 * 1024),
        &[rd_core::StatedHash {
            algorithm: "sha-256".to_owned(),
            value: "cd".repeat(32),
        }],
        Some((
            "sha-1".to_owned(),
            16 * 1024,
            vec!["0".repeat(40), "1".repeat(40)],
        )),
    )
    .expect("set")
}

#[tokio::test]
async fn a_candidate_with_mirrors_becomes_one_download_with_every_mirror_as_a_source() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (_, _, candidates) = harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::Manual,
            source_label: None,
            package_name: Some("Metalink".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None],
            file_names: vec![Some("image.iso".to_owned())],
            sizes: vec![None],
            requests: vec![None],
            body_refs: vec![None],
            urls: vec!["https://first.example/image.iso".parse().expect("url")],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    harness
        .database
        .set_candidate_source_set(candidates[0].id, metalink_set())
        .await
        .expect("set");

    let (_, packages) = get_json(&harness.router, "/api/v1/collector/packages").await;
    let ids: Vec<&str> = packages
        .as_array()
        .expect("packages")
        .iter()
        .filter_map(|package| package["id"].as_str())
        .collect();
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/packages/enqueue",
        json!({ "ids": ids, "paused": true }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    let downloads = downloads.as_array().expect("downloads");
    // One transfer, not one per mirror.
    assert_eq!(downloads.len(), 1, "{downloads:?}");
    let download = &downloads[0];
    // `ByteCount` travels as a string, so a size above 2^53 survives JavaScript.
    assert_eq!(download["total_bytes"], json!((32 * 1024).to_string()));
    assert_eq!(download["expected_checksum"]["algorithm"], "sha256");
    let id = download["id"].as_str().expect("id");

    let (status, sources) =
        get_json(&harness.router, &format!("/api/v1/downloads/{id}/sources")).await;
    assert_eq!(status, StatusCode::OK, "{sources}");
    let rows = sources["sources"].as_array().expect("sources");
    let order: Vec<(&str, &str)> = rows
        .iter()
        .map(|row| {
            (
                row["host"].as_str().unwrap_or_default(),
                row["state"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        order,
        [
            ("first.example", "ready"),
            ("second.example", "ready"),
            ("third.example", "unsupported"),
        ]
    );
    assert_eq!(rows[0]["location"], "de");
    assert_eq!(rows[0]["priority"], 1);
    // A signed query value is never shown.
    assert!(
        !rows[1]["url"]
            .as_str()
            .unwrap_or_default()
            .contains("abcdef0123456789"),
        "{rows:?}"
    );
    assert_eq!(sources["piece_hashes"]["pieces"], 2);
    assert_eq!(sources["piece_hashes"]["piece_length"], 16 * 1024);
}

#[tokio::test]
async fn a_download_with_a_single_address_has_no_sources() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/downloads",
        json!({ "url": "https://only.example/file.bin" }),
    )
    .await;
    assert!(status.is_success(), "{body}");
    let (_, downloads) = get_json(&harness.router, "/api/v1/downloads").await;
    let id = downloads[0]["id"].as_str().expect("id");
    let (status, sources) =
        get_json(&harness.router, &format!("/api/v1/downloads/{id}/sources")).await;
    assert_eq!(status, StatusCode::OK, "{sources}");
    assert_eq!(sources["sources"], json!([]));
    assert_eq!(sources["piece_hashes"], serde_json::Value::Null);

    let (status, _) = get_json(
        &harness.router,
        &format!("/api/v1/downloads/{}/sources", rd_core::DownloadId::new()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A plain HTTP listener that answers every request with a small file and records the path of
/// each request it receives.
async fn recording_listener() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    let paths = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&paths);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let mut buffer = [0_u8; 2048];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                seen.lock().expect("paths").push(path);
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\n\
                          content-length: 4\r\nconnection: close\r\n\r\nfile",
                    )
                    .await;
            });
        }
    });
    (port, paths)
}

/// RD-150-03: the LinkGrabber checks a fresh link at once. A Metalink relayed from a web page
/// names its first mirror at `127.0.0.1` — this service's own port, in the real attack — and
/// that link is marked `collector.check_internal_address` without a single request. The link
/// the person gave in the same batch is checked as always and reaches the same listener,
/// which is what makes the silence on the mirror's path mean something.
#[tokio::test]
async fn the_online_check_never_requests_a_relayed_metalinks_mirror_at_this_machine() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let (port, paths) = recording_listener().await;
    let own = format!("http://127.0.0.1:{port}/own.bin");
    let mirror = format!("http://127.0.0.1:{port}/mirror.bin");
    let (batch, _, candidates) = harness
        .database
        .add_collector_batch(rd_db::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: rd_core::IngressSource::BrowserExtension,
            source_label: None,
            package_name: Some("Metalink".to_owned()),
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            providers: vec![None, None],
            file_names: vec![Some("own.bin".to_owned()), Some("mirror.bin".to_owned())],
            sizes: vec![None, None],
            requests: vec![None, None],
            body_refs: vec![None, None],
            urls: vec![own.parse().expect("url"), mirror.parse().expect("url")],
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    let (own_id, mirror_id) = (candidates[0].id, candidates[1].id);
    let set = rd_core::SourceSet::checked([(mirror.clone(), None, None)], Some(4), &[], None)
        .expect("set");
    harness
        .database
        .set_candidate_source_set(mirror_id, set)
        .await
        .expect("set");
    // What the intake writes for a link a parser proposed out of a relayed document.
    harness
        .database
        .set_candidates_remote_reach(vec![mirror_id], false)
        .await
        .expect("reach");

    harness.link_check.check_batch(batch.id).await;
    let database = &harness.database;
    let checked = eventually(
        Duration::from_secs(10),
        "both links were checked",
        || async move {
            let candidates = database.list_candidates().await.ok()?;
            let settled = |id: rd_core::CandidateId| {
                candidates
                    .iter()
                    .find(|candidate| candidate.id == id)
                    .filter(|candidate| {
                        candidate.checked_at.is_some()
                            && candidate.state != rd_core::LinkCandidateState::Checking
                    })
            };
            Some((settled(own_id)?.clone(), settled(mirror_id)?.clone()))
        },
    )
    .await;
    let (own_checked, mirror_checked) = checked;
    assert_eq!(
        mirror_checked.error_code.as_deref(),
        Some("collector.check_internal_address"),
        "{mirror_checked:?}"
    );
    assert_ne!(
        own_checked.error_code.as_deref(),
        Some("collector.check_internal_address")
    );
    let paths = paths.lock().expect("paths").clone();
    assert!(paths.iter().any(|path| path == "/own.bin"), "{paths:?}");
    assert!(
        !paths.iter().any(|path| path == "/mirror.bin"),
        "the mirror was requested: {paths:?}"
    );
}
