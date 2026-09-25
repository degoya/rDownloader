//! The two Server-Sent-Event streams: the web interface's and the capture agent's.
//!
//! Both read the same bus and differ only in what they let through. The web stream shows a
//! subscriber the kinds its scopes cover; the capture stream is deliberately narrow, because a
//! capture token is a restricted credential (`capture:*`) meant for handing links in, and the
//! full bus carries download paths, account, proxy and credential changes. Two events go
//! through it: the intake a desktop notification needs, and -- stripped to a bare count -- the
//! captcha signal (RD-107-03).
//!
//! Both streams resume (RD-110-23). A connection that names the last id it saw in
//! `Last-Event-ID` is handed what the bus buffered after it, through the very same filter as
//! the live stream -- a replay must never show a connection an event it would not have been
//! shown live -- and is told with a marker when the buffer no longer reaches back that far.
//! Every stream opens with the `retry:` the service wants its clients to reconnect at.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    Extension,
    extract::State,
    http::HeaderMap,
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use futures_util::{Stream, StreamExt, stream};
use rd_core::{EventEnvelope, EventId, EventKind};
use rd_db::Replay;
use tokio_stream::wrappers::{BroadcastStream, errors::BroadcastStreamRecvError};

use crate::{AppState, auth::Granted};

/// How long a client waits before reconnecting, sent as the stream's `retry:` field.
///
/// Above the capture agent's own first backoff (2 s) and below its second, so a short outage
/// costs nothing and a long one no longer climbs to a minute between attempts; the agent keeps
/// the value undoubled (`Reconnect::requested`). A constant rather than a load measurement:
/// the mechanism is here, pacing by load would be its own finding with its own measurement.
pub const RECONNECT_AFTER: Duration = Duration::from_secs(5);

/// The event name a subscriber sees when the bus dropped messages before it read them.
///
/// Not an `EventKind`: it does not come off the bus, it says that part of the bus was missed.
const LAGGED_EVENT: &str = "stream.lagged";

/// The event name a subscriber sees when the id it resumed from is no longer in the buffer.
///
/// The same class of marker as [`LAGGED_EVENT`]: it names nothing from the bus and says that
/// everything since the client's last event is gone -- because the id fell out of the buffer,
/// because the service restarted, or because it never was an id. Silence would look exactly
/// like a quiet bus.
const EXPIRED_EVENT: &str = "stream.expired";

const LAST_EVENT_ID: &str = "last-event-id";

/// The event stream of the web interface.
///
/// Every event belongs to a scope (`rd_core::Scope::of_event`), and a subscriber sees the
/// kinds its own scopes cover: a read-only token gets queue and progress, a session gets the
/// whole bus. The filter lives here rather than in a second endpoint so both clients keep one
/// URL and one reconnect story.
///
/// The extension is missing only if this handler is ever mounted outside the session layer.
/// That would be a routing mistake, and the safe reading of it is an empty scope set -- a
/// silent stream rather than a stream of everything.
pub async fn events(
    State(state): State<AppState>,
    granted: Option<Extension<Granted>>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Shared rather than cloned per event: the closure runs once per message for the life of
    // the stream, and the scope set never changes after the request was authorised.
    let granted = Arc::new(granted.map_or_else(Granted::default, |Extension(granted)| granted));
    let frame = move |event: EventEnvelope| {
        granted
            .may_observe(&event.kind)
            .then(|| full_frame(&event))
            .flatten()
    };
    Sse::new(open(&state.database, &headers, frame)).keep_alive(KeepAlive::default())
}

/// The event stream a paired capture agent may subscribe to. See the module documentation
/// for why it carries two event kinds and nothing else.
pub async fn capture_events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    Sse::new(open(&state.database, &headers, capture_frame)).keep_alive(KeepAlive::default())
}

/// Opens one stream: the `retry:` hint, then the replay or the expiry marker, then the live
/// bus -- the replay and the bus through the same `frame` function, which is what keeps a
/// resumed connection from seeing anything the live one would have withheld.
fn open<F>(
    database: &rd_db::Database,
    headers: &HeaderMap,
    frame: F,
) -> impl Stream<Item = Result<Event, Infallible>> + use<F>
where
    F: Fn(EventEnvelope) -> Option<Event> + Clone + Send + 'static,
{
    let (replayed, live) = match resume_point(headers) {
        None => (Vec::new(), database.subscribe()),
        Some(point) => {
            let (replay, live) = match point.parse::<EventId>() {
                Ok(id) => database.resume(id),
                Err(_) => (Replay::Expired, database.subscribe()),
            };
            let replayed = match replay {
                Replay::Events(missed) => {
                    tracing::debug!(
                        last_event_id = %point,
                        missed = missed.len(),
                        "event stream resumed"
                    );
                    missed.into_iter().filter_map(frame.clone()).collect()
                }
                Replay::Expired => {
                    tracing::info!(
                        last_event_id = %point,
                        "event stream could not be resumed; sent an expiry marker"
                    );
                    vec![
                        Event::default()
                            .event(EXPIRED_EVENT)
                            .data(expired_payload(&point)),
                    ]
                }
            };
            (replayed, live)
        }
    };
    let opening = std::iter::once(Event::default().retry(RECONNECT_AFTER))
        .chain(replayed)
        .map(Ok);
    let live = BroadcastStream::new(live).filter_map(move |result| {
        let frame = frame.clone();
        async move {
            match received(result) {
                Ok(event) => frame(event).map(Ok),
                Err(marker) => Some(Ok(marker)),
            }
        }
    });
    stream::iter(opening).chain(live)
}

/// The id the client last saw, if it sent one.
fn resume_point(headers: &HeaderMap) -> Option<String> {
    headers
        .get(LAST_EVENT_ID)?
        .to_str()
        .ok()
        .map(str::trim)
        .filter(|point| !point.is_empty())
        .map(ToOwned::to_owned)
}

/// The body of the expiry marker: the id it could not resume from, which the client sent
/// itself, and nothing else.
fn expired_payload(point: &str) -> String {
    serde_json::json!({ "last_event_id": point }).to_string()
}

/// The body of the lag marker -- how many events were lost.
///
/// Split out because `Event` keeps its fields private, so this is the only part of the marker
/// a test can read.
fn lagged_payload(dropped: u64) -> String {
    serde_json::json!({ "dropped": dropped }).to_string()
}

/// One message off the bus: the event it carried, or the marker the subscriber must be sent
/// in place of the events it missed.
///
/// The bus is a `broadcast::channel(512)`, which overwrites the oldest entry once a subscriber
/// falls that far behind and reports it as `Lagged(n)` exactly once. Both streams used to read
/// that with `result.ok()?`, which discarded it like an uninteresting kind: the client lost
/// events with no error, no log and no way to know, and the interface then showed the state
/// from before the gap indefinitely. The marker tells it to refetch instead, and the stream
/// stays alive -- the receiver is usable again straight after a lag.
///
/// The marker deliberately carries no SSE `id`. It is not a bus event, and an id would move
/// the client's reconnect cursor past the very events that were lost. It also deliberately
/// skips the scope filter: a count of dropped messages names nothing, and a subscriber that
/// may not see an event kind still has to learn that its own view is stale.
fn received(
    result: Result<EventEnvelope, BroadcastStreamRecvError>,
) -> Result<EventEnvelope, Event> {
    match result {
        Ok(event) => Ok(event),
        Err(BroadcastStreamRecvError::Lagged(dropped)) => {
            tracing::warn!(
                dropped,
                "event subscriber fell behind; sent a resynchronise marker"
            );
            Err(Event::default()
                .event(LAGGED_EVENT)
                .data(lagged_payload(dropped)))
        }
    }
}

/// The frame the web stream sends for an event: the whole envelope.
fn full_frame(event: &EventEnvelope) -> Option<Event> {
    let data = serde_json::to_string(event).ok()?;
    Some(
        Event::default()
            .id(event.id.to_string())
            .event(event_name(&event.kind))
            .data(data),
    )
}

/// The frame the capture stream sends for an event, or `None` for a kind it does not carry.
fn capture_frame(event: EventEnvelope) -> Option<Event> {
    let data = match event.kind {
        EventKind::CollectorIntake => serde_json::to_string(&event).ok()?,
        EventKind::CaptchaChanged => {
            serde_json::to_string(&redacted_captcha_signal(&event)).ok()?
        }
        _ => return None,
    };
    Some(
        Event::default()
            .id(event.id.to_string())
            .event(event_name(&event.kind))
            .data(data),
    )
}

/// The captcha announcement as the capture surface may see it: how many widgets are waiting,
/// and nothing else.
///
/// The bus payload carries the full pending list -- image captchas as `data:` URIs, prompts,
/// page URLs, site keys. A capture token has no business reading any of that, so the agent is
/// told only that something changed and fetches `/api/v1/capture/captchas`, where the same
/// credential is checked again and the answer is already narrowed.
fn redacted_captcha_signal(event: &EventEnvelope) -> serde_json::Value {
    let widgets = event
        .payload
        .get("pending")
        .and_then(serde_json::Value::as_array)
        .map_or(0, |pending| {
            pending
                .iter()
                .filter(|entry| entry.get("site_key").is_some())
                .count()
        });
    serde_json::json!({
        "id": event.id,
        "kind": "captcha.changed",
        "payload": { "widgets": widgets }
    })
}

fn event_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::DownloadProgress => "download.progress",
        EventKind::DownloadState => "download.state",
        EventKind::PackageState => "package.state",
        EventKind::CollectorChanged => "collector.changed",
        EventKind::CollectorIntake => "collector.intake",
        EventKind::CategoryChanged => "category.changed",
        EventKind::HotFolderChanged => "hotfolder.changed",
        EventKind::CaptureChanged => "capture.changed",
        EventKind::AccountChanged => "account.changed",
        EventKind::AuthProfileChanged => "auth_profile.changed",
        EventKind::RemoteCredentialChanged => "remote_credential.changed",
        EventKind::ProxyChanged => "proxy.changed",
        EventKind::UsenetChanged => "usenet.changed",
        EventKind::PluginChanged => "plugin.changed",
        EventKind::PluginTrustChanged => "plugin_trust.changed",
        EventKind::PluginCatalogChanged => "plugin_catalog.changed",
        EventKind::PostprocessCatalogChanged => "postprocess_catalog.changed",
        EventKind::ManagedToolChanged => "managed_tool.changed",
        EventKind::StreamChanged => "stream.changed",
        EventKind::SubscriptionChanged => "subscription.changed",
        EventKind::PostprocessProgress => "postprocess.progress",
        EventKind::CaptchaChanged => "captcha.changed",
        EventKind::TorrentStats => "torrent.stats",
        EventKind::StorageCapacity => "storage.capacity",
        EventKind::BandwidthChanged => "bandwidth.changed",
        EventKind::PowerChanged => "power.changed",
        EventKind::NotificationChanged => "notification.changed",
        EventKind::AutomationChanged => "automation.changed",
        EventKind::ReconnectChanged => "reconnect.changed",
        EventKind::RemoteJobChanged => "remote_job.changed",
        EventKind::SiteRuleChanged => "site_rule.changed",
        EventKind::System => "system",
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use rd_core::{EventEnvelope, EventKind};
    use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

    use super::{
        capture_frame, expired_payload, lagged_payload, received, redacted_captcha_signal,
        resume_point,
    };

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
}
