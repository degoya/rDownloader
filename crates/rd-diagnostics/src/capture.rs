//! The `tracing` layer that feeds the log store.
//!
//! Two promises, both kept here and nowhere else:
//!
//! * **Nothing leaves this layer unredacted.** The message and every field value pass through
//!   `rd_core::redact_text`; a field whose *name* says it holds a credential — `password`,
//!   `api_key`, `authorization`, anything `rd_core::is_secret_parameter` knows — is replaced as
//!   a whole. The store, the API and the viewer only carry what this produced.
//! * **Logging never blocks.** The record goes into a bounded channel with `try_send`; when the
//!   sink is behind, the record is dropped and counted, and the thread that logged goes on.
//!   A download engine must not stall because the database is busy writing its own log.

use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::Utc;
use rd_core::{LogLevel, REDACTION_PLACEHOLDER, is_secret_parameter, redact_header_value};
use rd_db::NewLogRecord;
use tokio::sync::mpsc;
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// The fields that tie a record to the work it belongs to, most specific first.
pub const CORRELATION_FIELDS: &[&str] = &[
    "correlation_id",
    // The trace a request or a job belongs to (RD-110-03). Second because an explicit
    // `correlation_id` is somebody saying "these belong together" and outranks a derived id;
    // ahead of the entity ids because it is the one value that spans the API, the scheduler,
    // a resolver and post-processing.
    "trace_id",
    "request_id",
    "download_id",
    "package_id",
    "job_id",
];

/// Records the channel holds before the layer starts dropping.
///
/// Sized for a burst — a resolver failing on every link of a large package — not for a
/// sustained rate the sink cannot keep up with; that is what the drop counter reports.
pub const CHANNEL_CAPACITY: usize = 4096;

/// Longest message or field value stored. A page excerpt quoted into a warning is useful at
/// two kilobytes and only bloats the store beyond that.
pub const MAX_VALUE_LENGTH: usize = 2048;

/// Counters the API reports beside the records, so a viewer that shows a gap can say why.
#[derive(Debug, Default)]
pub struct CaptureStats {
    captured: AtomicU64,
    dropped: AtomicU64,
}

/// A point-in-time reading of [`CaptureStats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureSnapshot {
    /// Records handed to the channel since the process started.
    pub captured: u64,
    /// Records the layer had to drop because the channel was full or closed.
    pub dropped: u64,
}

impl CaptureStats {
    /// The counters as they stand.
    #[must_use]
    pub fn snapshot(&self) -> CaptureSnapshot {
        CaptureSnapshot {
            captured: self.captured.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}

static PROCESS_STATS: std::sync::OnceLock<Arc<CaptureStats>> = std::sync::OnceLock::new();

fn process_stats() -> Arc<CaptureStats> {
    Arc::clone(PROCESS_STATS.get_or_init(|| Arc::new(CaptureStats::default())))
}

/// The counters of the layer [`install`] created, or zeros when none was installed.
#[must_use]
pub fn snapshot() -> CaptureSnapshot {
    PROCESS_STATS
        .get()
        .map(|stats| stats.snapshot())
        .unwrap_or_default()
}

/// The receiving end of the channel, handed to `sink::spawn` once the database is open.
pub struct LogStream {
    pub(crate) receiver: mpsc::Receiver<NewLogRecord>,
}

impl LogStream {
    /// What is waiting right now, without blocking; for the sink and for tests.
    pub fn drain_ready(&mut self) -> Vec<NewLogRecord> {
        let mut records = Vec::new();
        while let Ok(record) = self.receiver.try_recv() {
            records.push(record);
        }
        records
    }
}

/// Creates the process-wide layer and its stream, with the counters [`snapshot`] reads.
#[must_use]
pub fn install() -> (LogCaptureLayer, LogStream) {
    LogCaptureLayer::with_stats(process_stats())
}

/// The layer itself; see the module documentation for what it promises.
pub struct LogCaptureLayer {
    sender: mpsc::Sender<NewLogRecord>,
    stats: Arc<CaptureStats>,
}

impl LogCaptureLayer {
    /// A layer with its own counters, for tests that must not read another test's numbers.
    #[must_use]
    pub fn with_stats(stats: Arc<CaptureStats>) -> (Self, LogStream) {
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        (Self { sender, stats }, LogStream { receiver })
    }

    /// The counters of this layer.
    #[must_use]
    pub fn stats(&self) -> CaptureSnapshot {
        self.stats.snapshot()
    }
}

/// The correlation fields an open span carries, kept in its extensions for the events inside.
struct SpanCorrelation(Vec<(&'static str, String)>);

/// Collects an event's or a span's fields, redacting as it goes.
#[derive(Default)]
struct FieldVisitor {
    message: Option<String>,
    fields: BTreeMap<String, String>,
}

impl FieldVisitor {
    fn push(&mut self, name: &str, raw: String) {
        let value = truncate(redact_field(name, &raw));
        if name == "message" {
            self.message = Some(value);
        } else {
            self.fields.insert(name.to_owned(), value);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.push(field.name(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.push(field.name(), value.to_owned());
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.push(field.name(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field.name(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field.name(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field.name(), value.to_string());
    }
}

/// Redacts one field: a credential-named field as a whole, any other through the text rules.
///
/// `redact_header_value` already answers the placeholder for `authorization`, `cookie` and the
/// other header names, and runs the text redaction otherwise; `is_secret_parameter` adds the
/// query-parameter vocabulary — `token`, `api_key`, `password`, the signature names — because a
/// field named like that holds the same kind of value whatever it was logged from.
#[must_use]
pub fn redact_field(name: &str, value: &str) -> String {
    if is_secret_parameter(name) {
        return REDACTION_PLACEHOLDER.to_owned();
    }
    redact_header_value(name, value)
}

fn truncate(mut value: String) -> String {
    if value.len() > MAX_VALUE_LENGTH {
        let mut end = MAX_VALUE_LENGTH;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        value.push_str("...");
    }
    value
}

fn level_of(level: &Level) -> LogLevel {
    match *level {
        Level::TRACE => LogLevel::Trace,
        Level::DEBUG => LogLevel::Debug,
        Level::INFO => LogLevel::Info,
        Level::WARN => LogLevel::Warn,
        Level::ERROR => LogLevel::Error,
    }
}

/// The first correlation field present, in [`CORRELATION_FIELDS`] order.
fn correlation_in(fields: &BTreeMap<String, String>) -> Option<String> {
    CORRELATION_FIELDS
        .iter()
        .find_map(|name| fields.get(*name).cloned())
}

impl<S> Layer<S> for LogCaptureLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        attributes.record(&mut visitor);
        let carried: Vec<(&'static str, String)> = CORRELATION_FIELDS
            .iter()
            .filter_map(|name| {
                visitor
                    .fields
                    .get(*name)
                    .map(|value| (*name, value.clone()))
            })
            .collect();
        if carried.is_empty() {
            return;
        }
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanCorrelation(carried));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        if *metadata.level() == Level::TRACE {
            return;
        }
        // The sink reports its own failures through `tracing`; capturing those would feed
        // them straight back into the channel it could not drain.
        if metadata.target().starts_with("rd_diagnostics") {
            return;
        }
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        let mut correlation_id = correlation_in(&visitor.fields);
        if let Some(scope) = ctx.event_scope(event) {
            for span in scope {
                let extensions = span.extensions();
                let Some(SpanCorrelation(carried)) = extensions.get::<SpanCorrelation>() else {
                    continue;
                };
                for (name, value) in carried {
                    visitor
                        .fields
                        .entry((*name).to_owned())
                        .or_insert_with(|| value.clone());
                }
                if correlation_id.is_none() {
                    correlation_id = correlation_in(&visitor.fields);
                }
            }
        }
        let code = visitor.fields.remove("code");
        let record = NewLogRecord {
            recorded_at: Utc::now(),
            level: level_of(metadata.level()),
            component: metadata.target().to_owned(),
            code,
            correlation_id,
            message: visitor.message.unwrap_or_default(),
            fields: visitor.fields,
        };
        match self.sender.try_send(record) {
            Ok(()) => {
                self.stats.captured.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
