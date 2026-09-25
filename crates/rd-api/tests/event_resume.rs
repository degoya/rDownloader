//! Resuming an event stream after a dropped connection (RD-110-23), and with it the intake that
//! arrived while the agent was away (RD-110-22).
//!
//! The streams are read a few frames at a time with a deadline: an SSE body never ends on its
//! own, so collecting it would never return.

mod common;

use std::{collections::BTreeMap, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use http_body_util::BodyExt;
use rd_core::{EventEnvelope, EventKind};
use serde_json::json;
use tower::ServiceExt;

const CAPTURE_ROUTE: &str = "/api/v1/capture/events";
const WEB_ROUTE: &str = "/api/v1/events";

/// One parsed frame: field name to value, several `data:` lines joined with a newline.
type Frame = BTreeMap<String, String>;

async fn open(router: &Router, route: &str, bearer: &str, last_event_id: Option<&str>) -> Body {
    let mut request = Request::builder()
        .uri(route)
        .header(header::HOST, "127.0.0.1:8710")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    if let Some(id) = last_event_id {
        request = request.header("Last-Event-ID", id);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body()
}

/// Reads frames until `wanted` have arrived or `patience` has run out, whichever is first.
/// Comment-only frames -- the keep-alive -- are not counted.
async fn frames(body: &mut Body, wanted: usize, patience: Duration) -> Vec<Frame> {
    let deadline = tokio::time::Instant::now() + patience;
    let mut pending: Vec<u8> = Vec::new();
    let mut frames = Vec::new();
    while frames.len() < wanted {
        let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(deadline, body.frame()).await else {
            break;
        };
        if let Some(data) = chunk.data_ref() {
            pending.extend_from_slice(data);
        }
        while let Some(end) = pending.windows(2).position(|window| window == b"\n\n") {
            let raw = String::from_utf8_lossy(&pending[..end]).into_owned();
            pending.drain(..end + 2);
            if let Some(frame) = parse(&raw) {
                frames.push(frame);
            }
        }
    }
    frames
}

fn parse(raw: &str) -> Option<Frame> {
    let mut frame = Frame::new();
    for line in raw.lines() {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        let entry = frame.entry(field.to_owned()).or_default();
        if !entry.is_empty() {
            entry.push('\n');
        }
        entry.push_str(value);
    }
    (!frame.is_empty()).then_some(frame)
}

fn field<'a>(frame: &'a Frame, name: &str) -> &'a str {
    frame.get(name).map_or("", String::as_str)
}

fn intake(candidates: u32) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::CollectorIntake,
        json!({ "candidate_count": candidates, "package_count": 1, "source": "api" }),
    )
}

/// RD-110-22 through RD-110-23: what the agent missed is replayed, exactly that, and nothing
/// the capture stream would not have carried live. The capture batch is the real intake, the
/// progress event is one the capture stream never carries, and the captcha announcement is one
/// it carries only as a count -- a replay that handed over the bus payload would be a leak the
/// live stream had already closed.
#[tokio::test]
async fn an_intake_that_arrived_while_the_agent_was_away_is_replayed_on_reconnect() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let router = &harness.router;

    let mut first = open(router, CAPTURE_ROUTE, common::CAPTURE_BEARER, None).await;
    harness.database.broadcast(intake(1));
    let opening = frames(&mut first, 2, Duration::from_secs(5)).await;
    assert_eq!(opening.len(), 2, "{opening:?}");
    assert_eq!(
        field(&opening[0], "retry"),
        "5000",
        "the service paces the reconnect: {opening:?}"
    );
    assert_eq!(field(&opening[1], "event"), "collector.intake");
    let held = opening[1].get("id").cloned().expect("an id to resume from");
    drop(first);

    let (status, payload) = common::post_capture(
        router,
        json!({
            "text": "https://files.example.com/missed.pdf",
            "source": "browser_extension",
            "source_label": "Browser"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{payload}");
    harness.database.broadcast(EventEnvelope::new(
        EventKind::DownloadProgress,
        json!({ "download_id": "not-for-the-agent" }),
    ));
    harness.database.broadcast(EventEnvelope::new(
        EventKind::CaptchaChanged,
        json!({ "pending": [{
            "id": "a", "kind": "turnstile", "site_key": "0x4AAA",
            "page_url": "https://ddownload.com/login.html", "host": "ddownload.com"
        }]}),
    ));

    let mut second = open(router, CAPTURE_ROUTE, common::CAPTURE_BEARER, Some(&held)).await;
    let replay = frames(&mut second, 3, Duration::from_secs(5)).await;
    assert_eq!(replay.len(), 3, "{replay:?}");
    assert_eq!(field(&replay[0], "retry"), "5000");
    assert_eq!(field(&replay[1], "event"), "collector.intake", "{replay:?}");
    assert!(
        field(&replay[1], "data").contains("\"candidate_count\":1"),
        "{replay:?}"
    );
    assert_ne!(
        field(&replay[1], "id"),
        held,
        "the held event itself is not replayed"
    );
    assert_eq!(field(&replay[2], "event"), "captcha.changed", "{replay:?}");
    let captcha = field(&replay[2], "data");
    assert!(captcha.contains("\"widgets\":1"), "{captcha}");
    for leaked in ["0x4AAA", "ddownload.com", "page_url"] {
        assert!(
            !captcha.contains(leaked),
            "the replay leaked {leaked}: {captcha}"
        );
    }
    let more = frames(&mut second, 1, Duration::from_millis(300)).await;
    assert!(
        more.is_empty(),
        "the replay carried more than what was missed: {more:?}"
    );
    let latest = field(&replay[2], "id").to_owned();
    drop(second);

    // Nothing missed, nothing replayed: only the retry hint opens the stream.
    let mut third = open(router, CAPTURE_ROUTE, common::CAPTURE_BEARER, Some(&latest)).await;
    let quiet = frames(&mut third, 2, Duration::from_millis(500)).await;
    assert_eq!(
        quiet.len(),
        1,
        "a reconnect with nothing missed replayed: {quiet:?}"
    );
    assert_eq!(field(&quiet[0], "retry"), "5000");
}

/// An id the buffer does not hold -- after a restart, or too old -- gets a marker that says
/// so, then the live stream. Silence would look exactly like a quiet bus.
#[tokio::test]
async fn an_id_the_buffer_no_longer_holds_is_answered_with_an_expired_marker() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::test_harness(directory.path()).await;
    let unknown = rd_core::EventId::new().to_string();

    for (route, bearer) in [
        (CAPTURE_ROUTE, common::CAPTURE_BEARER),
        (WEB_ROUTE, common::API_BEARER),
    ] {
        let mut stream = open(&harness.router, route, bearer, Some(&unknown)).await;
        let opening = frames(&mut stream, 2, Duration::from_secs(5)).await;
        assert_eq!(opening.len(), 2, "{route}: {opening:?}");
        assert_eq!(field(&opening[0], "retry"), "5000", "{route}");
        assert_eq!(
            field(&opening[1], "event"),
            "stream.expired",
            "{route}: {opening:?}"
        );
        assert!(
            field(&opening[1], "data").contains(&unknown),
            "{route}: the marker names the id it could not resume from: {opening:?}"
        );
        assert!(
            !opening[1].contains_key("id"),
            "{route}: a marker must not move the reconnect cursor: {opening:?}"
        );

        // The stream is live after the marker.
        harness.database.broadcast(intake(2));
        let live = frames(&mut stream, 1, Duration::from_secs(5)).await;
        assert_eq!(live.len(), 1, "{route}: {live:?}");
        assert_eq!(field(&live[0], "event"), "collector.intake", "{route}");
    }

    // An id that is not even a UUID is answered the same way, never with silence.
    let mut stream = open(
        &harness.router,
        CAPTURE_ROUTE,
        common::CAPTURE_BEARER,
        Some("42"),
    )
    .await;
    let opening = frames(&mut stream, 2, Duration::from_secs(5)).await;
    assert_eq!(opening.len(), 2, "{opening:?}");
    assert_eq!(field(&opening[1], "event"), "stream.expired", "{opening:?}");
}

/// The web stream resumes under the same scope filter it streams live under: a read-only
/// token that could not have seen an account change live is not handed it in a replay.
#[tokio::test]
async fn the_web_stream_replays_under_its_scope_filter() {
    let directory = tempfile::tempdir().expect("tempdir");
    let harness = common::auth_harness(directory.path()).await;
    let router = &harness.router;

    let mut first = open(router, WEB_ROUTE, common::READ_BEARER, None).await;
    harness.database.broadcast(EventEnvelope::new(
        EventKind::DownloadProgress,
        json!({ "download_id": "a" }),
    ));
    let opening = frames(&mut first, 2, Duration::from_secs(5)).await;
    assert_eq!(opening.len(), 2, "{opening:?}");
    assert_eq!(field(&opening[1], "event"), "download.progress");
    let held = field(&opening[1], "id").to_owned();
    drop(first);

    harness.database.broadcast(EventEnvelope::new(
        EventKind::AccountChanged,
        json!({ "account_id": "secret-account" }),
    ));
    harness.database.broadcast(EventEnvelope::new(
        EventKind::DownloadState,
        json!({ "download_id": "a", "state": "completed" }),
    ));

    let mut second = open(router, WEB_ROUTE, common::READ_BEARER, Some(&held)).await;
    let replay = frames(&mut second, 3, Duration::from_millis(800)).await;
    assert_eq!(replay.len(), 2, "{replay:?}");
    assert_eq!(field(&replay[0], "retry"), "5000");
    assert_eq!(field(&replay[1], "event"), "download.state", "{replay:?}");
    assert!(
        !replay
            .iter()
            .any(|frame| field(frame, "data").contains("secret-account")),
        "the replay handed a read-only token an account change: {replay:?}"
    );
}
