use axum::http::{HeaderMap, HeaderValue};
use rd_core::{EventEnvelope, EventKind};
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use super::{
    capture_frame, expired_payload, lagged_payload, received, redacted_captcha_signal,
    resume_point, with_quiet_until, with_toast_owner,
};

/// One surface per intake (RD-1260-04): the tab learns whether an agent shows the toast,
/// and the agent when quiet hours end; every other event passes unchanged.
#[test]
fn an_intake_names_who_shows_it_and_until_when_it_waits() {
    let intake = || EventEnvelope::new(EventKind::CollectorIntake, serde_json::json!({}));
    assert_eq!(
        with_toast_owner(intake(), true).payload["agent_toast"],
        true
    );
    assert_eq!(
        with_toast_owner(intake(), false).payload["agent_toast"],
        false
    );
    let until = chrono::Utc::now();
    assert!(with_quiet_until(intake(), Some(until)).payload["quiet_until"].is_string());
    assert!(
        with_quiet_until(intake(), None)
            .payload
            .get("quiet_until")
            .is_none()
    );
    let other = EventEnvelope::new(EventKind::CaptchaChanged, serde_json::json!({}));
    assert!(
        with_toast_owner(other, true)
            .payload
            .get("agent_toast")
            .is_none()
    );
}

/// A subscriber that fell behind used to be handed nothing at all, which is
/// indistinguishable from a quiet bus. It has to be told, and told how much it missed.
#[test]
fn a_lagging_subscriber_is_sent_a_resynchronise_marker() {
    assert!(
        received(Err(BroadcastStreamRecvError::Lagged(7))).is_err(),
        "a lag has to produce a marker, not a skipped item"
    );
    assert_eq!(lagged_payload(7), r#"{"dropped":7}"#);

    let event = EventEnvelope::new(EventKind::DownloadProgress, serde_json::json!({}));
    let id = event.id;
    let Ok(passed) = received(Ok(event)) else {
        panic!("a normal event has to pass through unchanged")
    };
    assert_eq!(passed.id, id);
}

/// A capture token must learn that a captcha is waiting and nothing more. The bus payload
/// carries the image itself, the prompt, the page and the site key; every one of those has
/// to be gone by the time it reaches the agent's stream.
#[test]
fn the_capture_stream_reduces_a_captcha_announcement_to_a_count() {
    let event = EventEnvelope::new(
        EventKind::CaptchaChanged,
        serde_json::json!({ "pending": [
            { "id": "a", "kind": "turnstile", "site_key": "0x4AAA",
              "page_url": "https://ddownload.com/login.html", "host": "ddownload.com" },
            { "id": "b", "kind": "image", "image": "data:image/png;base64,Qk0=",
              "prompt": "Type the code" },
        ]}),
    );

    let signal = redacted_captcha_signal(&event);

    assert_eq!(signal["payload"]["widgets"], 1);
    let serialised = signal.to_string();
    for leaked in ["0x4AAA", "ddownload.com", "data:image/png", "Type the code"] {
        assert!(
            !serialised.contains(leaked),
            "the capture stream leaked {leaked}: {serialised}"
        );
    }
}

/// An announcement with nothing waiting is still forwarded, so an agent that just closed
/// its window learns the queue is empty instead of waiting for a timeout.
#[test]
fn an_empty_queue_is_announced_as_zero_widgets() {
    let event = EventEnvelope::new(
        EventKind::CaptchaChanged,
        serde_json::json!({ "pending": [] }),
    );

    assert_eq!(redacted_captcha_signal(&event)["payload"]["widgets"], 0);
}

/// The one filter for both the live stream and the replay: a kind the capture stream does
/// not carry yields no frame, whichever way it arrives.
#[test]
fn the_capture_filter_is_one_function_for_live_and_replayed_events() {
    let carried = EventEnvelope::new(EventKind::CollectorIntake, serde_json::json!({}));
    let withheld = EventEnvelope::new(EventKind::AccountChanged, serde_json::json!({}));
    assert!(capture_frame(carried).is_some());
    assert!(capture_frame(withheld).is_none());
}

#[test]
fn the_resume_point_is_the_trimmed_header_or_nothing() {
    let mut headers = HeaderMap::new();
    assert_eq!(resume_point(&headers), None);
    headers.insert("Last-Event-ID", HeaderValue::from_static("  abc "));
    assert_eq!(resume_point(&headers).as_deref(), Some("abc"));
    headers.insert("Last-Event-ID", HeaderValue::from_static(""));
    assert_eq!(
        resume_point(&headers),
        None,
        "an empty header is no resume point"
    );
}

#[test]
fn the_expiry_marker_names_only_the_id_the_client_sent() {
    assert_eq!(expired_payload("42"), r#"{"last_event_id":"42"}"#);
}
