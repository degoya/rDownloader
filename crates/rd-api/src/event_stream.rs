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
//!
//! Both streams end when the credential they were opened with stops standing -- signed out,
//! revoked, expired, narrowed (`rd_api_core::stream_standing`, security audit 2026-09-30). The
//! client reconnects as it would after any drop, and the reconnect is authorised afresh.
//!
//! Both streams also end when the service stops (`AppState::shutdown`). The graceful stop waits
//! for every open response to finish, and an event stream never finishes by itself: the update
//! dialog's own tab held one open, the service accepted the stop and never ended, and the
//! updater gave up after 120 seconds (RD-180-02, live test 2026-10-01).

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
pub(crate) const RECONNECT_AFTER: Duration = Duration::from_secs(5);

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
pub(crate) async fn events(
    State(state): State<AppState>,
    granted: Option<Extension<Granted>>,
    crate::client::ThisMachine(from_this_machine): crate::client::ThisMachine,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Shared rather than cloned per event: the closure runs once per message for the life of
    // the stream. The scope set is fixed for that life, which is why the stream ends rather
    // than widening or narrowing once the credential behind it changes.
    let granted = Arc::new(granted.map_or_else(Granted::default, |Extension(granted)| granted));
    let lapsed = rd_api_core::stream_standing::api_lapsed(
        state.clone(),
        headers.clone(),
        from_this_machine,
        granted.scopes().to_vec(),
    );
    let agents = state.capture_agents.clone();
    let frame = move |event: EventEnvelope| {
        granted
            .may_observe(&event.kind)
            .then(|| full_frame(&with_toast_owner(event, agents.any_connected())))
            .flatten()
    };
    let stopping = state.shutdown.clone().cancelled_owned();
    Sse::new(
        open(&state.database, &headers, frame)
            .take_until(lapsed)
            .take_until(stopping),
    )
    .keep_alive(KeepAlive::default())
}

/// The event stream a paired capture agent may subscribe to. See the module documentation
/// for why it carries two event kinds and nothing else.
///
/// While it is open the agent counts as running, with the version it named in its
/// `User-Agent` (`rd_api_core::capture_agents`, RD-190-07): the stream holds the connection,
/// and the entry ends with it.
pub(crate) async fn capture_events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let lapsed = rd_api_core::stream_standing::capture_lapsed(state.clone(), headers.clone());
    let stopping = state.shutdown.clone().cancelled_owned();
    let connection = rd_api_core::capture_agents::connect(&state, &headers).await;
    let power = state.power.clone();
    let frame = move |event: EventEnvelope| {
        capture_frame(with_quiet_until(
            event,
            power.notifications_quiet_until(chrono::Utc::now()),
        ))
    };
    Sse::new(
        open(&state.database, &headers, frame)
            .take_until(lapsed)
            .take_until(stopping)
            .map(move |frame| {
                // Owned by the stream, so it is dropped with the response.
                let _held = &connection;
                frame
            }),
    )
    .keep_alive(KeepAlive::default())
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

/// An intake as the web interface is sent it, saying whether a connected capture agent shows
/// the desktop toast for it (`agent_toast`): one surface per event, so the tab leaves it to the
/// agent while one runs (RD-1260-04). One the extension announces itself carries
/// `extension_notice` from the store, and both leave it alone (RD-1270-05).
fn with_toast_owner(mut event: EventEnvelope, agent_connected: bool) -> EventEnvelope {
    if event.kind == EventKind::CollectorIntake
        && let Some(payload) = event.payload.as_object_mut()
    {
        payload.insert("agent_toast".to_owned(), agent_connected.into());
    }
    event
}

/// An intake as the capture agent is sent it while quiet hours hold notifications back: with
/// `quiet_until`, the moment the agent shows what it held as one toast (RD-1260-04).
fn with_quiet_until(
    mut event: EventEnvelope,
    until: Option<chrono::DateTime<chrono::Utc>>,
) -> EventEnvelope {
    if event.kind == EventKind::CollectorIntake
        && let Some(until) = until
        && let Some(payload) = event.payload.as_object_mut()
    {
        payload.insert("quiet_until".to_owned(), serde_json::json!(until));
    }
    event
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
        EventKind::QueueStopMark => "queue.stop_mark",
        EventKind::System => "system",
    }
}

#[cfg(test)]
#[path = "event_stream_tests.rs"]
mod tests;
