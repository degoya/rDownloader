//! Switching a transfer service off, end to end.
//!
//! Two promises are being checked. A link of a switched-off kind is refused at intake rather
//! than queued, because a row that can never run is worse than a refusal: nothing in the
//! queue explains it. And a fresh installation shares no torrent data at all — not after a
//! download finishes, and not while one is running.

mod common;

use axum::http::StatusCode;
use common::{get_json, post_json, put_json, test_harness, test_router};
use serde_json::json;
use tracing_subscriber::{layer::SubscriberExt, registry};

/// Reads the settings document, applies a patch, and writes it back.
async fn update(router: &axum::Router, patch: serde_json::Value) {
    let (_, mut settings) = get_json(router, "/api/v1/settings").await;
    // Writing the document re-applies the login switch from it, and the harness only set the
    // in-memory flag. Without this every request after the first write needs a session.
    settings["admin_login_disabled"] = serde_json::Value::Bool(true);
    for (key, value) in patch.as_object().expect("patch object") {
        settings[key] = value.clone();
    }
    let (status, body) = put_json(router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn every_service_is_on_by_default() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (_, settings) = get_json(&router, "/api/v1/settings").await;
    for key in [
        "torrent_service_enabled",
        "usenet_service_enabled",
        "media_service_enabled",
        "gallery_service_enabled",
        "recording_service_enabled",
        "remote_service_enabled",
    ] {
        assert_eq!(settings[key], true, "{key} should default to on");
    }
}

#[tokio::test]
async fn a_fresh_installation_shares_no_torrent_data() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;

    let (_, settings) = get_json(&router, "/api/v1/settings").await;
    // Two separate things, both off. Seeding covers what happens after a download finishes;
    // sharing covers whether anything is uploaded at all, including while downloading.
    assert_eq!(settings["torrent_seeding_enabled"], false, "{settings}");
    assert_eq!(settings["torrent_sharing_enabled"], false, "{settings}");
}

#[tokio::test]
async fn a_magnet_is_refused_while_the_torrent_service_is_off() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    update(&router, json!({ "torrent_service_enabled": false })).await;

    let (status, body) = post_json(
        &router,
        "/api/v1/collector/batches",
        json!({
            "source": "manual",
            "text": "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "collector.all_links_disabled", "{body}");

    // And nothing was created on the way to the refusal.
    let (_, candidates) = get_json(&router, "/api/v1/collector/candidates").await;
    assert_eq!(candidates.as_array().map(Vec::len), Some(0), "{candidates}");
}

#[tokio::test]
async fn a_link_of_a_running_service_still_gets_through() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    update(&router, json!({ "torrent_service_enabled": false })).await;

    // Switching one service off must not touch the others.
    let (status, body) = post_json(
        &router,
        "/api/v1/collector/batches",
        json!({ "source": "manual", "text": "https://example.com/file.bin" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test]
async fn a_mixed_paste_keeps_what_it_can_and_reports_the_rest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    update(&router, json!({ "torrent_service_enabled": false })).await;

    let (status, body) = post_json(
        &router,
        "/api/v1/collector/batches",
        json!({
            "source": "manual",
            "text": "https://example.com/file.bin\nmagnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Reported rather than silently dropped: the person pasting has to be able to see that
    // one of their links did not make it.
    assert_eq!(body["skipped_disabled"], 1, "{body}");
    assert_eq!(
        body["candidates"].as_array().map(Vec::len),
        Some(1),
        "{body}"
    );
}

#[tokio::test]
async fn a_torrent_upload_is_refused_too() {
    let directory = tempfile::tempdir().expect("tempdir");
    let router = test_router(directory.path()).await;
    update(&router, json!({ "torrent_service_enabled": false })).await;

    // The `.torrent` upload does not go through the LinkGrabber intake, so it needs its own
    // check — otherwise the one path that bypasses intake stays open.
    const BOUNDARY: &str = "----rdtest";
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.torrent\"\r\n\r\nd4:infod4:name1:xee\r\n--{BOUNDARY}--\r\n"
    );
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/v1/torrents/import")
        .header(axum::http::header::HOST, "127.0.0.1:8710")
        .header(
            axum::http::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(axum::body::Body::from(body))
        .expect("request");
    let response = tower::ServiceExt::oneshot(router.clone(), request)
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Collects the `setting_target` of every settings-reader report, i.e. every "a stored value in
/// service.settings has the wrong type or shape" ERROR.
#[derive(Clone, Default)]
struct SettingsReports(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for SettingsReports {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct Target(Option<String>);
        impl tracing::field::Visit for Target {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "setting_target" {
                    self.0 = Some(value.to_owned());
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        let mut target = Target(None);
        event.record(&mut target);
        if let Some(target) = target.0 {
            self.0.lock().expect("reports").push(target);
        }
    }
}

/// Saving the settings form stores an unset `excluded_domains_file` as `null`, and the next
/// LinkGrabber intake read that as a malformed string and logged an ERROR (RD-120-48). Saved
/// here through the real `PUT /api/v1/settings`, then read by a real intake.
#[tokio::test]
async fn an_ordinary_settings_save_does_not_break_the_blocklist_reader() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    update(&harness.router, json!({})).await;
    let stored = harness
        .database
        .get_setting("service.settings")
        .await
        .expect("read")
        .expect("the save wrote the document");
    // The premise: the writer keeps storing `null` for "no file", and that stays valid.
    assert!(stored["excluded_domains_file"].is_null(), "{stored}");

    let reports = SettingsReports::default();
    let _guard = tracing::subscriber::set_default(registry().with(reports.clone()));
    let (status, body) = post_json(
        &harness.router,
        "/api/v1/collector/batches",
        json!({ "source": "manual", "text": "https://files.example/archive.bin" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let reported = reports.0.lock().expect("reports").clone();
    assert!(
        reported.is_empty(),
        "an ordinary save must leave nothing the settings reader rejects: {reported:?}"
    );
}
