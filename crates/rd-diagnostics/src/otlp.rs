//! Optional trace export over OTLP/HTTP (RD-110-03).
//!
//! ## Why no OpenTelemetry SDK
//!
//! The same decision RD-110-01 took for Prometheus, for the same reason. What is needed here
//! is a JSON document posted to one URL: `opentelemetry`, `opentelemetry_sdk`,
//! `opentelemetry-otlp`, `tonic` and `prost` would bring a second tracing pipeline with its
//! own provider, its own sampler, its own batch processor and its own idea of a global
//! subscriber, next to the `tracing` layer stack this service already runs. The encoding
//! below is the proto3 JSON mapping of `ExportTraceServiceRequest` — the wire format OTLP
//! specifies for `Content-Type: application/json` — and it has a golden test. `reqwest` is
//! already in the tree.
//!
//! ## What it promises
//!
//! * **Off by default.** [`set_active`] is false until the settings say otherwise, and while
//!   it is false the layer does nothing but load one atomic.
//! * **It never blocks and never retries.** Spans go into a bounded channel with `try_send`;
//!   a full channel drops and counts. A failed export drops its batch. A collector that is
//!   down, slow or gone costs nothing but a counter and one log line.
//! * **Nothing sensitive leaves.** Every attribute passes `capture::redact_field` on the way
//!   in, the same function the log store's records pass, so a signed URL in a span field is
//!   `[redacted]` before it is ever encoded.

use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// Spans the channel holds before the layer starts dropping.
pub const CHANNEL_CAPACITY: usize = 2048;
/// Spans per export request.
pub const EXPORT_BATCH: usize = 512;
/// How long a span waits in the buffer at most.
pub const EXPORT_INTERVAL: Duration = Duration::from_secs(2);
/// The `service.name` resource attribute every exported span carries.
pub const SERVICE_NAME: &str = "rdownloader";

static ACTIVE: AtomicBool = AtomicBool::new(false);
static ENQUEUED: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static EXPORTED: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);

/// Whether the layer should build spans at all.
///
/// Read on every span close, so the cost of the feature while it is off is one relaxed atomic
/// load per span and no allocation.
#[must_use]
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Switches the layer on or off. Called by the exporter task whenever the settings change.
pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Relaxed);
}

/// What the exporter has done so far, for the diagnostics page and for tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportSnapshot {
    pub enqueued: u64,
    pub dropped: u64,
    pub exported: u64,
    pub failed: u64,
}

/// The counters as they stand.
#[must_use]
pub fn snapshot() -> ExportSnapshot {
    ExportSnapshot {
        enqueued: ENQUEUED.load(Ordering::Relaxed),
        dropped: DROPPED.load(Ordering::Relaxed),
        exported: EXPORTED.load(Ordering::Relaxed),
        failed: FAILED.load(Ordering::Relaxed),
    }
}

/// One finished span, in the shape the encoder needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpanRecord {
    /// Lowercase hex, 32 characters.
    pub trace_id: String,
    /// Lowercase hex, 16 characters.
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub start_unix_nano: u64,
    pub end_unix_nano: u64,
    /// Whether the span ended in an error, which becomes OTLP status code `2`.
    pub failed: bool,
    /// Already redacted; see the module documentation.
    pub attributes: BTreeMap<String, String>,
}

/// The receiving end of the span channel, handed to [`spawn`] once the database is open.
pub struct SpanStream {
    receiver: mpsc::Receiver<SpanRecord>,
}

impl SpanStream {
    /// What is waiting right now, without blocking; for tests.
    pub fn drain_ready(&mut self) -> Vec<SpanRecord> {
        let mut spans = Vec::new();
        while let Ok(span) = self.receiver.try_recv() {
            spans.push(span);
        }
        spans
    }
}

/// The sending end the layer holds.
#[derive(Clone)]
pub struct SpanSink {
    sender: mpsc::Sender<SpanRecord>,
}

impl SpanSink {
    /// Offers a span. Never waits: a full channel drops and counts, because a download must
    /// not stall behind a collector.
    pub fn offer(&self, span: SpanRecord) {
        match self.sender.try_send(span) {
            Ok(()) => {
                ENQUEUED.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Creates the channel the layer writes into and the exporter reads from.
#[must_use]
pub fn channel() -> (SpanSink, SpanStream) {
    let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
    (SpanSink { sender }, SpanStream { receiver })
}

fn attribute(key: &str, value: &str) -> Value {
    json!({ "key": key, "value": { "stringValue": value } })
}

/// The proto3 JSON mapping of one `ExportTraceServiceRequest`.
///
/// Frozen shape: a collector parses this, so the key spelling is the contract and the golden
/// test in `tests/otlp.rs` is what holds it. `startTimeUnixNano` and `endTimeUnixNano` are
/// strings because proto3 JSON maps 64-bit integers to strings — a number would silently lose
/// precision in a JSON parser that uses doubles.
#[must_use]
pub fn encode(spans: &[SpanRecord], version: &str) -> Value {
    let encoded: Vec<Value> = spans
        .iter()
        .map(|span| {
            let mut value = json!({
                "traceId": span.trace_id,
                "spanId": span.span_id,
                "name": span.name,
                "kind": 1,
                "startTimeUnixNano": span.start_unix_nano.to_string(),
                "endTimeUnixNano": span.end_unix_nano.to_string(),
                "attributes": span
                    .attributes
                    .iter()
                    .map(|(key, value)| attribute(key, value))
                    .collect::<Vec<_>>(),
                "status": { "code": if span.failed { 2 } else { 0 } },
            });
            if let Some(parent) = span.parent_span_id.as_deref()
                && let Some(object) = value.as_object_mut()
            {
                object.insert("parentSpanId".to_owned(), Value::String(parent.to_owned()));
            }
            value
        })
        .collect();
    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    attribute("service.name", SERVICE_NAME),
                    attribute("service.version", version),
                ]
            },
            "scopeSpans": [{
                "scope": { "name": SERVICE_NAME, "version": version },
                "spans": encoded
            }]
        }]
    })
}

/// Posts one batch. The caller treats any error as "dropped", never as "retry".
pub async fn export(
    client: &reqwest::Client,
    endpoint: &str,
    timeout: Duration,
    spans: &[SpanRecord],
    version: &str,
) -> Result<()> {
    if spans.is_empty() {
        return Ok(());
    }
    let response = client
        .post(endpoint.trim())
        .timeout(timeout)
        .json(&encode(spans, version))
        .send()
        .await
        .context("post spans to the OTLP endpoint")?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("the OTLP endpoint answered {status}");
    }
    Ok(())
}

/// Records the outcome of one batch, so the counters say the same thing the task does.
pub fn note_export(spans: u64, ok: bool) {
    if ok {
        EXPORTED.fetch_add(spans, Ordering::Relaxed);
    } else {
        FAILED.fetch_add(spans, Ordering::Relaxed);
    }
}

/// Starts the exporter for the life of the process. It ends when every sender is gone.
///
/// The settings are read once per batch rather than once at startup, so switching the export
/// on or off, or pointing it somewhere else, takes effect without a restart — the same rule
/// the log retention sweep follows.
pub fn spawn(
    stream: SpanStream,
    database: rd_db::Database,
    version: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run(stream, database, version))
}

async fn run(mut stream: SpanStream, database: rd_db::Database, version: String) {
    let client = match reqwest::Client::builder().build() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "the OTLP exporter could not build its HTTP client");
            set_active(false);
            return;
        }
    };
    let mut buffer: Vec<SpanRecord> = Vec::with_capacity(EXPORT_BATCH);
    let mut tick = tokio::time::interval(EXPORT_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            received = stream.receiver.recv() => match received {
                Some(span) => {
                    buffer.push(span);
                    if buffer.len() >= EXPORT_BATCH {
                        flush(&client, &database, &mut buffer, &version).await;
                    }
                }
                None => {
                    flush(&client, &database, &mut buffer, &version).await;
                    return;
                }
            },
            _ = tick.tick() => {
                flush(&client, &database, &mut buffer, &version).await;
            }
        }
    }
}

/// Reads the settings, switches the layer accordingly and sends whatever is buffered.
async fn flush(
    client: &reqwest::Client,
    database: &rd_db::Database,
    buffer: &mut Vec<SpanRecord>,
    version: &str,
) {
    let settings: rd_core::OtelSettings = match database.service_settings_or_default().await {
        Ok(settings) => settings,
        Err(error) => {
            // The settings are unreadable, so the answer to "should this be exporting" is
            // unknown. Unknown means off: sending to a stale endpoint is worse than a gap.
            tracing::warn!(%error, "could not read the trace export settings");
            set_active(false);
            buffer.clear();
            return;
        }
    };
    let active = settings.is_active();
    set_active(active);
    if !active {
        buffer.clear();
        return;
    }
    if buffer.is_empty() {
        return;
    }
    let spans = std::mem::take(buffer);
    let timeout = Duration::from_secs(u64::from(settings.otlp_timeout_seconds.max(1)));
    match export(client, &settings.otlp_endpoint, timeout, &spans, version).await {
        Ok(()) => note_export(spans.len() as u64, true),
        Err(error) => {
            note_export(spans.len() as u64, false);
            // Said once per batch and never retried. A collector that is down must cost a
            // line in the log and nothing else; a retry queue here would grow in memory for
            // exactly as long as the outage lasts.
            tracing::warn!(%error, spans = spans.len(), "a batch of spans was not exported");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SpanRecord, encode};

    #[test]
    fn a_span_without_a_parent_carries_no_parent_key() {
        let span = SpanRecord {
            trace_id: "a".repeat(32),
            span_id: "b".repeat(16),
            name: "http.request".to_owned(),
            ..SpanRecord::default()
        };
        let document = encode(&[span], "1.1.0");
        let encoded = &document["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert!(encoded.get("parentSpanId").is_none());
        assert_eq!(encoded["status"]["code"], 0);
    }

    #[test]
    fn a_failed_span_reports_the_error_status() {
        let span = SpanRecord {
            trace_id: "a".repeat(32),
            span_id: "b".repeat(16),
            parent_span_id: Some("c".repeat(16)),
            failed: true,
            ..SpanRecord::default()
        };
        let document = encode(&[span], "1.1.0");
        let encoded = &document["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(encoded["status"]["code"], 2);
        assert_eq!(encoded["parentSpanId"], "c".repeat(16));
    }
}
