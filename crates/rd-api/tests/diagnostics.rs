//! The log viewer and the diagnostic bundle over REST (RD-110-02).
//!
//! The store and the layer are covered in `crates/rd-diagnostics/tests`; what is checked here
//! is the contract a client sees: a secret logged anywhere never reaches the viewer, the read
//! filters and refuses what it should, and a bundle exists only after its preview was approved
//! for exactly the inventory the person saw.

mod common;

use std::sync::{Arc, OnceLock};

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use common::{get_json, post_json, put_json, test_harness, test_router};
use http_body_util::BodyExt;
use rd_diagnostics::capture::{CaptureStats, LogCaptureLayer};
use serde_json::json;
use tower::ServiceExt;
use tracing::subscriber::with_default;
use tracing_subscriber::{layer::SubscriberExt, registry};

const CANARIES: &[&str] = &[
    "sk-live-4242",
    "hunter2",
    "bearer-token-value",
    "sigv4-signature-value",
];

/// The process-wide data directory, set once for this test binary: bundles are written under
/// it, and `rd_core::set_data_directory` is a `OnceLock` the first caller owns.
fn data_directory() -> &'static std::path::Path {
    static DATA: OnceLock<tempfile::TempDir> = OnceLock::new();
    let directory = DATA.get_or_init(|| tempfile::tempdir().expect("data directory"));
    rd_core::set_data_directory(directory.path());
    directory.path()
}

/// Logs through a private capture layer straight into the harness's store.
async fn log_into(database: &rd_db::Database, emit: impl FnOnce()) {
    let (layer, mut stream) = LogCaptureLayer::with_stats(Arc::new(CaptureStats::default()));
    with_default(registry().with(layer), emit);
    rd_diagnostics::sink::drain_now(&mut stream, database)
        .await
        .expect("drain");
}

#[tokio::test]
async fn the_viewer_never_shows_a_known_secret_pattern() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    log_into(&harness.database, || {
        tracing::warn!(
            url = "https://cdn.example/f?token=sk-live-4242&X-Amz-Signature=sigv4-signature-value",
            authorization = "Bearer bearer-token-value",
            "refused for ftp://bob:hunter2@files.example/pub"
        );
        tracing::error!(code = "http.status", password = "hunter2", "login failed");
    })
    .await;

    for uri in [
        "/api/v1/diagnostics/logs",
        "/api/v1/diagnostics/logs?search=refused",
        "/api/v1/diagnostics/logs?level=error&code=http.status",
    ] {
        let (status, body) = get_json(&harness.router, uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let text = body.to_string();
        for canary in CANARIES {
            assert!(!text.contains(canary), "{uri} shows {canary}: {text}");
        }
        assert!(
            !body["records"].as_array().expect("records").is_empty(),
            "{uri}"
        );
    }
    let (_, body) = get_json(&harness.router, "/api/v1/diagnostics/logs").await;
    assert_eq!(body["records"][0]["code"], "http.status");
    assert_eq!(body["records"][0]["fields"]["password"], "[redacted]");
    assert!(
        body["records"][1]["fields"]["url"]
            .as_str()
            .is_some_and(|url| url.contains("token=%5Bredacted%5D")),
        "{body}"
    );
    assert_eq!(body["total"], 2);
    assert_eq!(body["retention"]["records"], 20_000);
    assert_eq!(body["retention"]["days"], 14);
}

#[tokio::test]
async fn the_log_read_filters_pages_and_refuses_a_bad_query() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    log_into(&harness.database, || {
        for index in 0..5 {
            tracing::info!(download_id = "dl-1", index, "step");
        }
        tracing::warn!("slow");
    })
    .await;

    let (status, body) = get_json(&harness.router, "/api/v1/diagnostics/logs?limit=2").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["records"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["full_page"], true, "a full page says so");
    let oldest_shown = body["records"][1]["id"].as_i64().expect("id");
    let (_, next) = get_json(
        &harness.router,
        &format!("/api/v1/diagnostics/logs?limit=10&before_id={oldest_shown}"),
    )
    .await;
    assert_eq!(next["records"].as_array().map(Vec::len), Some(4));
    assert_eq!(next["full_page"], false);

    let (_, correlated) = get_json(
        &harness.router,
        "/api/v1/diagnostics/logs?correlation_id=dl-1&component=rd_api",
    )
    .await;
    assert_eq!(
        correlated["records"].as_array().map(Vec::len),
        Some(0),
        "{correlated}"
    );
    let (_, correlated) = get_json(
        &harness.router,
        "/api/v1/diagnostics/logs?correlation_id=dl-1",
    )
    .await;
    assert_eq!(correlated["records"].as_array().map(Vec::len), Some(5));

    for uri in [
        "/api/v1/diagnostics/logs?level=loud",
        "/api/v1/diagnostics/logs?limit=501",
        "/api/v1/diagnostics/logs?limit=0",
        "/api/v1/diagnostics/logs?since=yesterday",
    ] {
        let (status, body) = get_json(&harness.router, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert_eq!(body["code"], "diagnostics.invalid_query", "{uri}");
    }
}

#[tokio::test]
async fn the_preview_inventory_is_deterministic() {
    data_directory();
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (status, first) = get_json(&router, "/api/v1/diagnostics/bundle/preview").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let (_, second) = get_json(&router, "/api/v1/diagnostics/bundle/preview").await;
    assert_eq!(first, second);
    let ids: Vec<&str> = first["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "versions",
            "configuration",
            "system-checks",
            "doctor",
            "recent-errors"
        ]
    );
    assert!(
        first["digest"]
            .as_str()
            .is_some_and(|digest| digest.len() == 64)
    );
    assert!(
        first["directory"]
            .as_str()
            .is_some_and(|dir| dir.ends_with("diagnostics"))
    );
    assert!(!first["excluded"].as_array().expect("excluded").is_empty());
}

#[tokio::test]
async fn a_bundle_is_only_written_after_its_preview_was_approved() {
    let data = data_directory();
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    log_into(&harness.database, || {
        tracing::error!(
            password = "hunter2",
            "canary https://h.example/f?token=sk-live-4242"
        );
    })
    .await;
    let router = &harness.router;

    let (status, body) = post_json(
        router,
        "/api/v1/diagnostics/bundle",
        json!({ "approved": false, "digest": "", "entries": ["versions"] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "diagnostics.approval_required");

    let (status, body) = post_json(
        router,
        "/api/v1/diagnostics/bundle",
        json!({ "approved": true, "digest": "not-what-was-shown", "entries": ["versions"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "diagnostics.preview_stale");

    let (_, preview) = get_json(router, "/api/v1/diagnostics/bundle/preview").await;
    let digest = preview["digest"].as_str().expect("digest").to_owned();
    let (status, body) = post_json(
        router,
        "/api/v1/diagnostics/bundle",
        json!({ "approved": true, "digest": digest, "entries": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "diagnostics.approval_required");

    let entries: Vec<&str> = preview["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    let (status, created) = post_json(
        router,
        "/api/v1/diagnostics/bundle",
        json!({ "approved": true, "digest": digest, "entries": entries }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let file_name = created["file_name"].as_str().expect("file name");
    assert!(
        rd_diagnostics::bundle::is_file_name(file_name),
        "{file_name}"
    );
    let path = std::path::PathBuf::from(created["path"].as_str().expect("path"));
    assert!(
        path.starts_with(data.join("diagnostics")),
        "{}",
        path.display()
    );
    let on_disk = std::fs::read(&path).expect("bundle on disk");
    assert_eq!(
        on_disk.len() as u64,
        created["bytes"].as_u64().expect("bytes")
    );
    assert_eq!(created["manifest"]["inventory_digest"], preview["digest"]);
    assert_eq!(
        created["manifest"]["entries"].as_array().map(Vec::len),
        Some(5)
    );

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/api/v1/diagnostics/bundles/{file_name}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/zip")
    );
    let downloaded = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(downloaded.as_ref(), on_disk.as_slice());

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(on_disk)).expect("zip");
    let mut names: Vec<String> = (0..archive.len())
        .map(|index| archive.by_index(index).expect("entry").name().to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "configuration.json",
            "doctor.txt",
            "manifest.json",
            "recent-errors.json",
            "system-checks.json",
            "versions.json"
        ]
    );
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).expect("entry");
        let mut text = String::new();
        std::io::Read::read_to_string(&mut entry, &mut text).expect("read");
        for canary in CANARIES {
            assert!(!text.contains(canary), "{} carries {canary}", entry.name());
        }
    }
}

#[tokio::test]
async fn a_bundle_name_that_was_not_generated_is_not_opened() {
    data_directory();
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    for name in [
        "rdownloader.sqlite3",
        "rdownloader-diagnostics-20260920T123000Z.zip.bak",
        "..%2Frdownloader-diagnostics-20260920T123000Z.zip",
    ] {
        let (status, body) =
            get_json(&router, &format!("/api/v1/diagnostics/bundles/{name}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}: {body}");
        assert_eq!(body["code"], "diagnostics.bundle_not_found", "{name}");
    }
}

#[tokio::test]
async fn retention_is_saved_with_the_settings_and_validated_there() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    let (_, mut settings) = get_json(&router, "/api/v1/settings").await;
    // Written back as the other settings tests do it: the harness reaches its routes without
    // a password only while the administrator login stays disabled in the document.
    settings["admin_login_disabled"] = json!(true);
    assert_eq!(settings["log_retention_records"], 20_000);
    assert_eq!(settings["log_retention_days"], 14);

    settings["log_retention_records"] = json!(10);
    let (status, body) = put_json(&router, "/api/v1/settings", settings.clone()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "settings.log_retention_invalid");

    settings["log_retention_records"] = json!(5_000);
    settings["log_retention_days"] = json!(7);
    let (status, body) = put_json(&router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, logs) = get_json(&router, "/api/v1/diagnostics/logs").await;
    assert_eq!(status, StatusCode::OK, "{logs}");
    assert_eq!(logs["retention"]["records"], 5_000, "{logs}");
    assert_eq!(logs["retention"]["days"], 7);
}
