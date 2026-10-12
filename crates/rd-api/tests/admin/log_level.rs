//! The log level and a debug window over REST (RD-1260-12).
//!
//! The filter itself is covered in `crates/rd-diagnostics/tests/log_filter.rs`; what is checked
//! here is what a client sees: a level chosen through the route holds from the next line, a
//! window opens only its area and comes back after a restart inside it, `RUST_LOG` locks both
//! and is named, and the refusals carry their codes.

use crate::common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::{delete_json, get_json, put_json, test_harness};
use rd_diagnostics::{
    LogStream,
    capture::{CaptureStats, LogCaptureLayer},
    log_filter,
};
use serde_json::json;
use tracing_subscriber::{layer::SubscriberExt, registry};

/// The service's subscriber in small: the service's filter, swapped by the routes, in front of
/// a capture layer. `rust_log` stands for the environment the service started in.
fn listen(rust_log: Option<&str>) -> (LogStream, tracing::subscriber::DefaultGuard) {
    let filter = log_filter::install(rust_log.map(str::to_owned));
    let (capture, stream) = LogCaptureLayer::with_stats(Arc::new(CaptureStats::default()));
    let guard = tracing::subscriber::set_default(registry().with(filter).with(capture));
    (stream, guard)
}

/// The probe lines this test wrote that reached the store, leaving out what the service logged.
fn probes(stream: &mut LogStream) -> Vec<String> {
    stream
        .drain_ready()
        .into_iter()
        .map(|record| record.message)
        .filter(|message| message.starts_with("probe "))
        .collect()
}

#[tokio::test]
async fn a_level_chosen_through_the_route_holds_from_the_next_line() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let router = &harness.router;
    let (mut stream, _guard) = listen(None);
    // The harness switches the login off in memory only; the route saves the stored document,
    // so that document has to say so too, as the other settings tests write it.
    let (_, mut settings) = get_json(router, "/api/v1/settings").await;
    settings["admin_login_disabled"] = json!(true);
    let (status, body) = put_json(router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, state) = get_json(router, "/api/v1/diagnostics/log-level").await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["level"], "info");
    assert!(state["debug"].is_null(), "{state}");
    assert!(state["rust_log"].is_null(), "{state}");
    assert_eq!(state["minutes"], json!([15, 30, 60]));
    assert_eq!(state["areas"].as_array().map(Vec::len), Some(9));

    tracing::debug!(target: "rd_http::engine", "probe debug at info");
    let (status, state) = put_json(
        router,
        "/api/v1/diagnostics/log-level",
        json!({ "level": "debug" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["level"], "debug");
    tracing::debug!(target: "rd_http::engine", "probe debug at debug");

    // The shortcut writes the settings document: the settings view reads the same level.
    let (status, mut settings) = get_json(router, "/api/v1/settings").await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["log_level"], "debug");
    settings["log_level"] = json!("warn");
    let (status, body) = put_json(router, "/api/v1/settings", settings).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    tracing::info!(target: "rd_http::engine", "probe info at warn");
    tracing::warn!(target: "rd_http::engine", "probe warn at warn");

    assert_eq!(
        probes(&mut stream),
        ["probe debug at debug", "probe warn at warn"]
    );
}

#[tokio::test]
async fn a_debug_window_opens_its_area_comes_back_after_a_restart_and_ends_on_request() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let router = &harness.router;
    let (mut stream, guard) = listen(None);

    let (status, state) = put_json(
        router,
        "/api/v1/diagnostics/log-level/debug",
        json!({ "area": "usenet", "minutes": 15 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert_eq!(state["level"], "info");
    assert_eq!(state["debug"]["area"], "usenet");
    let until: chrono::DateTime<chrono::Utc> = state["debug"]["until"]
        .as_str()
        .and_then(|until| until.parse().ok())
        .expect("until");
    let left = until - chrono::Utc::now();
    assert!(
        left > chrono::Duration::minutes(14) && left <= chrono::Duration::minutes(15),
        "{left}"
    );
    tracing::debug!(target: "rd_usenet::nntp", "probe usenet debug");
    tracing::debug!(target: "rd_torrent::session", "probe torrent debug");
    assert_eq!(probes(&mut stream), ["probe usenet debug"]);

    // A restart inside the window: a fresh filter at `info`, then what the start applies.
    drop(guard);
    let (mut stream, _guard) = listen(None);
    tracing::debug!(target: "rd_usenet::nntp", "probe before the start applied");
    log_filter::apply_stored(&harness.database).await;
    tracing::debug!(target: "rd_usenet::nntp", "probe after the start applied");
    assert_eq!(probes(&mut stream), ["probe after the start applied"]);
    // And one after it: the stored window is over and opens nothing.
    let stored = log_filter::stored_window(&harness.database).await;
    assert!(stored.is_some());
    assert_eq!(
        log_filter::reopened(stored, until + chrono::Duration::seconds(1)),
        None
    );

    let (status, state) = delete_json(router, "/api/v1/diagnostics/log-level/debug").await;
    assert_eq!(status, StatusCode::OK, "{state}");
    assert!(state["debug"].is_null(), "{state}");
    tracing::debug!(target: "rd_usenet::nntp", "probe usenet after the end");
    assert!(probes(&mut stream).is_empty());
    assert_eq!(log_filter::stored_window(&harness.database).await, None);
}

#[tokio::test]
async fn a_window_outside_what_is_offered_is_refused_with_its_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let router = &harness.router;
    let _listening = listen(None);

    let (status, body) = put_json(
        router,
        "/api/v1/diagnostics/log-level/debug",
        json!({ "area": "http", "minutes": 20 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "diagnostics.debug_minutes_invalid");

    for request in [
        json!({ "area": "usenet", "plugin_id": "019d0000-0000-7000-8000-00000000abcd", "minutes": 15 }),
        json!({ "area": "plugin", "plugin_id": "019d0000-0000-7000-8000-00000000abcd", "minutes": 15 }),
        json!({ "area": "plugin", "plugin_id": "x],rd_=trace", "minutes": 15 }),
    ] {
        let (status, body) = put_json(
            router,
            "/api/v1/diagnostics/log-level/debug",
            request.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{request}: {body}");
        assert_eq!(
            body["code"], "diagnostics.debug_plugin_invalid",
            "{request}"
        );
    }
    let (_, state) = get_json(router, "/api/v1/diagnostics/log-level").await;
    assert!(state["debug"].is_null(), "nothing was opened: {state}");
}

#[tokio::test]
async fn rust_log_locks_the_level_and_the_window_and_is_named() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = test_harness(directory.path()).await;
    let router = &harness.router;
    let (mut stream, _guard) = listen(Some("rd_http=debug"));

    let (_, state) = get_json(router, "/api/v1/diagnostics/log-level").await;
    assert_eq!(state["rust_log"], "rd_http=debug");

    let (status, body) = put_json(
        router,
        "/api/v1/diagnostics/log-level",
        json!({ "level": "error" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "diagnostics.log_level_locked");
    assert_eq!(body["params"]["rust_log"], "rd_http=debug");

    let (status, body) = put_json(
        router,
        "/api/v1/diagnostics/log-level/debug",
        json!({ "area": "usenet", "minutes": 15 }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "diagnostics.log_level_locked");

    tracing::debug!(target: "rd_http::engine", "probe what RUST_LOG asks for");
    tracing::info!(target: "rd_usenet::nntp", "probe what RUST_LOG leaves out");
    assert_eq!(probes(&mut stream), ["probe what RUST_LOG asks for"]);
}
