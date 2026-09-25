//! The `tracing` layer that turns closed spans into OTLP spans (RD-110-03).
//!
//! It exports **only spans that belong to a trace**: a span carrying a `trace_id` field, and
//! any span opened inside one. Everything else — and that is almost every span in the
//! workspace — is ignored without an allocation. This is what keeps the feature from becoming
//! a second, unbounded copy of the log store: the crates that want a trace say so, by opening
//! a span with `rd_core::TRACE_FIELD` on it.
//!
//! The context comes from the span's own `trace_id` field when it has one and from the
//! nearest ancestor that had one otherwise, so `#[instrument]`ed work inside a request or a
//! job span joins that trace with nothing to pass along. Span ids are derived
//! (`TraceContext::child`) from the parent and the `tracing` span id, which is unique within
//! the process, so two concurrent spans of the same name never collide.
//!
//! Every attribute goes through [`crate::capture::redact_field`] — the same function the log
//! records pass — before it is stored, so nothing unredacted can reach a collector.

use std::{
    collections::BTreeMap,
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

use rd_core::{TRACE_FIELD, TraceContext};
use tracing::{
    Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use crate::{
    capture::redact_field,
    otlp::{SpanRecord, SpanSink, is_active},
};

/// Longest attribute value exported; the log store's limit, for the same reason.
pub const MAX_ATTRIBUTE_LENGTH: usize = 2048;

/// What an open span holds until it closes.
struct SpanState {
    context: TraceContext,
    parent_span_id: Option<String>,
    name: &'static str,
    start_unix_nano: u64,
    attributes: BTreeMap<String, String>,
}

/// Collects a span's fields, redacting as it goes.
#[derive(Default)]
struct AttributeVisitor {
    attributes: BTreeMap<String, String>,
}

impl AttributeVisitor {
    fn push(&mut self, name: &str, raw: String) {
        let mut value = redact_field(name, &raw);
        if value.len() > MAX_ATTRIBUTE_LENGTH {
            let mut end = MAX_ATTRIBUTE_LENGTH;
            while end > 0 && !value.is_char_boundary(end) {
                end -= 1;
            }
            value.truncate(end);
        }
        self.attributes.insert(name.to_owned(), value);
    }
}

impl Visit for AttributeVisitor {
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

fn now_unix_nano() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        })
}

/// The context a span with this `trace_id` field belongs to, if the value is one.
fn context_from_field(attributes: &BTreeMap<String, String>, span_id: u64) -> Option<TraceContext> {
    let value = attributes.get(TRACE_FIELD)?;
    let mut trace_id = [0_u8; 16];
    hex::decode_to_slice(value.trim(), &mut trace_id).ok()?;
    let root = TraceContext::new(trace_id, [0, 0, 0, 0, 0, 0, 0, 1], true)?;
    Some(root.child(&span_id.to_string()))
}

/// Builds OTLP spans from the spans that carry a trace context. See the module documentation.
pub struct TraceExportLayer {
    sink: SpanSink,
}

impl TraceExportLayer {
    #[must_use]
    pub fn new(sink: SpanSink) -> Self {
        Self { sink }
    }
}

impl<S> Layer<S> for TraceExportLayer
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut visitor = AttributeVisitor::default();
        attributes.record(&mut visitor);

        let parent = span.parent().and_then(|parent| {
            parent
                .extensions()
                .get::<SpanState>()
                .map(|state| state.context)
        });
        let context = match context_from_field(&visitor.attributes, id.into_u64()) {
            Some(context) => context,
            None => match parent {
                // Inside a traced span: join it, with a span id of this span's own.
                Some(parent) => parent.child(&id.into_u64().to_string()),
                // Not part of any trace. Nothing is stored, so `on_close` does nothing and
                // the span costs one lookup.
                None => return,
            },
        };
        span.extensions_mut().insert(SpanState {
            context,
            parent_span_id: parent.map(|parent| parent.span_id_hex()),
            name: span.name(),
            start_unix_nano: now_unix_nano(),
            attributes: visitor.attributes,
        });
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        let Some(state) = extensions.get_mut::<SpanState>() else {
            return;
        };
        let mut visitor = AttributeVisitor::default();
        values.record(&mut visitor);
        state.attributes.extend(visitor.attributes);
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        // Checked first: while the export is off this is one relaxed load and nothing else.
        if !is_active() {
            return;
        }
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let mut extensions = span.extensions_mut();
        let Some(state) = extensions.remove::<SpanState>() else {
            return;
        };
        let failed = state.attributes.contains_key("error")
            || state
                .attributes
                .get("outcome")
                .is_some_and(|outcome| outcome == "failure");
        self.sink.offer(SpanRecord {
            trace_id: state.context.trace_id_hex(),
            span_id: state.context.span_id_hex(),
            parent_span_id: state.parent_span_id,
            name: state.name.to_owned(),
            start_unix_nano: state.start_unix_nano,
            end_unix_nano: now_unix_nano(),
            failed,
            attributes: state.attributes,
        });
    }
}
