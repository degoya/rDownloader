//! The level and the debug window decide what reaches the layers behind the filter, from the
//! next line on and without a restart (RD-1260-12).

use std::sync::Arc;

use chrono::{Duration, Utc};
use rd_core::{LogDebugArea, LogDebugWindow, ServiceLogLevel};
use rd_diagnostics::{
    capture::{CaptureStats, LogCaptureLayer},
    log_filter::{ApplyError, Controller},
};
use tracing_subscriber::{layer::SubscriberExt, registry};

/// A subscriber like the service's: the filter first, the capture layer behind it.
fn subscriber(
    rust_log: Option<&str>,
) -> (
    Controller,
    impl tracing::Subscriber + Send + Sync,
    rd_diagnostics::LogStream,
) {
    let (controller, filter) = Controller::with_layer(rust_log.map(str::to_owned));
    let (capture, stream) = LogCaptureLayer::with_stats(Arc::new(CaptureStats::default()));
    (controller, registry().with(filter).with(capture), stream)
}

fn messages(stream: &mut rd_diagnostics::LogStream) -> Vec<String> {
    stream
        .drain_ready()
        .into_iter()
        .map(|record| record.message)
        .collect()
}

fn window(area: LogDebugArea, until: chrono::DateTime<Utc>) -> LogDebugWindow {
    LogDebugWindow {
        area,
        plugin_id: None,
        until,
    }
}

#[test]
fn a_chosen_level_holds_from_the_next_line() {
    let (controller, subscriber, mut stream) = subscriber(None);
    let _default = tracing::subscriber::set_default(subscriber);

    tracing::debug!(target: "rd_http::engine", "debug before");
    tracing::info!(target: "rd_http::engine", "info before");
    controller
        .apply(ServiceLogLevel::Debug, None)
        .expect("applied");
    tracing::debug!(target: "rd_http::engine", "debug after");
    controller
        .apply(ServiceLogLevel::Warn, None)
        .expect("applied");
    tracing::info!(target: "rd_http::engine", "info at warn");
    tracing::warn!(target: "rd_http::engine", "warn at warn");

    assert_eq!(
        messages(&mut stream),
        ["info before", "debug after", "warn at warn"]
    );
    assert_eq!(controller.state().level, ServiceLogLevel::Warn);
}

#[test]
fn a_debug_window_opens_only_its_area() {
    let (controller, subscriber, mut stream) = subscriber(None);
    let _default = tracing::subscriber::set_default(subscriber);
    controller
        .apply(
            ServiceLogLevel::Info,
            Some(window(
                LogDebugArea::Usenet,
                Utc::now() + Duration::minutes(15),
            )),
        )
        .expect("applied");

    tracing::debug!(target: "rd_usenet::nntp", "usenet debug");
    tracing::debug!(target: "rd_torrent::session", "torrent debug");
    tracing::info!(target: "rd_torrent::session", "torrent info");

    assert_eq!(messages(&mut stream), ["usenet debug", "torrent info"]);
    assert_eq!(
        controller.state().debug.map(|window| window.area),
        Some(LogDebugArea::Usenet)
    );
}

/// The window's end shortened to a fraction of a second stands for the quarter of an hour:
/// the timer is what closes it, not the next request.
#[tokio::test]
async fn a_debug_window_closes_by_itself_at_its_end() {
    let (controller, subscriber, mut stream) = subscriber(None);
    let _default = tracing::subscriber::set_default(subscriber);
    controller
        .apply(
            ServiceLogLevel::Info,
            Some(window(
                LogDebugArea::Scheduler,
                Utc::now() + Duration::milliseconds(200),
            )),
        )
        .expect("applied");
    tracing::debug!(target: "rd_scheduler::runner", "inside the window");

    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    tracing::debug!(target: "rd_scheduler::runner", "after the window");
    tracing::info!(target: "rd_scheduler::runner", "info after the window");

    assert_eq!(
        messages(&mut stream),
        ["inside the window", "info after the window"]
    );
    assert_eq!(controller.state().debug, None);
    assert_eq!(controller.state().level, ServiceLogLevel::Info);
}

#[test]
fn a_debug_line_is_redacted_like_an_info_line() {
    let (controller, subscriber, mut stream) = subscriber(None);
    let _default = tracing::subscriber::set_default(subscriber);
    controller
        .apply(ServiceLogLevel::Debug, None)
        .expect("applied");

    tracing::debug!(
        target: "rd_http::engine",
        password = "hunter2",
        url = "https://cdn.example.com/f.bin?token=sk-live-4242",
        "fetching https://host.example/x?api_key=sk-live-4242"
    );

    let records = stream.drain_ready();
    assert_eq!(records.len(), 1, "the debug line was recorded");
    let rendered = serde_json::to_string(&records).expect("json");
    for canary in ["hunter2", "sk-live-4242"] {
        assert!(!rendered.contains(canary), "{canary} survived: {rendered}");
    }
}

#[test]
fn rust_log_wins_and_is_named() {
    let (controller, subscriber, mut stream) = subscriber(Some("rd_http=debug"));
    let _default = tracing::subscriber::set_default(subscriber);

    assert_eq!(
        controller.apply(ServiceLogLevel::Error, None),
        Err(ApplyError::LockedByEnvironment)
    );
    tracing::debug!(target: "rd_http::engine", "still debug");
    tracing::info!(target: "rd_usenet::nntp", "not in RUST_LOG");

    assert_eq!(messages(&mut stream), ["still debug"]);
    let state = controller.state();
    assert_eq!(state.rust_log.as_deref(), Some("rd_http=debug"));
    assert_eq!(
        state.level,
        ServiceLogLevel::Error,
        "recorded, not in force"
    );
}

#[test]
fn a_rust_log_that_does_not_parse_leaves_the_settings_in_charge() {
    let (controller, _subscriber, _stream) = subscriber(Some("rd_http=[nonsense"));
    assert_eq!(controller.state().rust_log, None);
    assert!(controller.apply(ServiceLogLevel::Warn, None).is_ok());
}
