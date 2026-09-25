//! Trace context: the identity a piece of work carries through the service (RD-110-03).
//!
//! One rule, and it is what makes the rest cheap: **the carrier is a `tracing` span field**,
//! not a task-local and not a column. `rd-diagnostics` already walks an event's span scope and
//! copies the correlation fields an ancestor carried (`capture::SpanCorrelation`), so a span
//! opened with `trace_id` puts that id on every record written inside it — through the API
//! handler, through the scheduler's runner, through a resolver call and through
//! post-processing — without a single function growing a parameter.
//!
//! Two kinds of context exist, deliberately:
//!
//! * A **request** context, from the caller's `traceparent` header when it sent one and a
//!   fresh root otherwise. It lives exactly as long as the request.
//! * A **job** context, [`TraceContext::for_job`], derived from the job's own identity with a
//!   hash. Queued work outlives the request that enqueued it — the scheduler picks a download
//!   up minutes later, in another task, possibly after a restart — so it does not inherit the
//!   request's trace. Deriving it instead means every span about download `42`, in every crate
//!   and across restarts, shares one trace id without anything being stored or passed.
//!
//! No part of this carries a URL, a file name or a credential. A trace id is sixteen bytes of
//! hash and says nothing about what it identifies.

use std::fmt;

use serde::{Deserialize, Serialize};

use sha2::{Digest, Sha256};

/// The domain string of a derived job trace, so an id derived here can never collide with one
/// derived for another purpose from the same bytes.
const JOB_DOMAIN: &str = "rdownloader.trace.job.v1";

/// The only `traceparent` version this service writes or accepts.
const VERSION: &str = "00";

/// The `sampled` flag of the W3C trace flags byte.
const FLAG_SAMPLED: u8 = 0x01;

/// A W3C trace context: which trace this work belongs to, and which span inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceContext {
    trace_id: [u8; 16],
    span_id: [u8; 8],
    sampled: bool,
}

impl TraceContext {
    /// A context from raw ids. Both must be non-zero; the W3C specification says an all-zero
    /// id is invalid, and an exporter that receives one drops the span silently.
    #[must_use]
    pub fn new(trace_id: [u8; 16], span_id: [u8; 8], sampled: bool) -> Option<Self> {
        if trace_id == [0; 16] || span_id == [0; 8] {
            return None;
        }
        Some(Self {
            trace_id,
            span_id,
            sampled,
        })
    }

    /// A fresh root, seeded from the clock and a counter.
    ///
    /// Not from `rand`: this crate carries no random-number generator and does not want one
    /// for an identifier that has to be unique rather than unguessable. A trace id is not a
    /// secret and grants nothing; what it must not do is repeat, which a monotonic counter
    /// mixed into a hash with the process start and the current nanosecond does not.
    #[must_use]
    pub fn root() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let mut hasher = Sha256::new();
        hasher.update(b"rdownloader.trace.root.v1");
        hasher.update(seq.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        hasher.update(std::process::id().to_be_bytes());
        Self::from_digest(&hasher.finalize(), true)
    }

    /// The context of one job, derived from what the job *is* rather than from who asked.
    ///
    /// `kind` is the family — `"download"`, `"package"`, `"postprocess"` — and `id` the
    /// identifier inside it. The same pair always yields the same trace id, so the scheduler,
    /// a resolver and post-processing agree without being told.
    #[must_use]
    pub fn for_job(kind: &str, id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(JOB_DOMAIN.as_bytes());
        hasher.update([kind.len() as u8]);
        hasher.update(kind.as_bytes());
        hasher.update(id.as_bytes());
        Self::from_digest(&hasher.finalize(), true)
    }

    fn from_digest(digest: &[u8], sampled: bool) -> Self {
        let mut trace_id = [0_u8; 16];
        let mut span_id = [0_u8; 8];
        trace_id.copy_from_slice(&digest[..16]);
        span_id.copy_from_slice(&digest[16..24]);
        // A hash is never all zeroes in practice, but "in practice" is not a guarantee, and a
        // zero id is the one value the specification forbids.
        if trace_id == [0; 16] {
            trace_id[15] = 1;
        }
        if span_id == [0; 8] {
            span_id[7] = 1;
        }
        Self {
            trace_id,
            span_id,
            sampled,
        }
    }

    /// A child span inside the same trace, with a span id derived from this one and `name`.
    #[must_use]
    pub fn child(&self, name: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"rdownloader.trace.child.v1");
        hasher.update(self.trace_id);
        hasher.update(self.span_id);
        hasher.update(name.as_bytes());
        let digest = hasher.finalize();
        let mut span_id = [0_u8; 8];
        span_id.copy_from_slice(&digest[..8]);
        if span_id == [0; 8] {
            span_id[7] = 1;
        }
        Self {
            trace_id: self.trace_id,
            span_id,
            sampled: self.sampled,
        }
    }

    #[must_use]
    pub fn trace_id(&self) -> [u8; 16] {
        self.trace_id
    }

    #[must_use]
    pub fn span_id(&self) -> [u8; 8] {
        self.span_id
    }

    #[must_use]
    pub fn sampled(&self) -> bool {
        self.sampled
    }

    /// The lowercase hex trace id: what the log store's `correlation_id` column holds and what
    /// a person pastes into the viewer's filter.
    #[must_use]
    pub fn trace_id_hex(&self) -> String {
        hex::encode(self.trace_id)
    }

    #[must_use]
    pub fn span_id_hex(&self) -> String {
        hex::encode(self.span_id)
    }

    /// The `traceparent` header value for this context.
    #[must_use]
    pub fn traceparent(&self) -> String {
        let flags = if self.sampled { FLAG_SAMPLED } else { 0 };
        format!(
            "{VERSION}-{}-{}-{:02x}",
            self.trace_id_hex(),
            self.span_id_hex(),
            flags
        )
    }

    /// Reads a `traceparent` header, or `None` when it is not one this service understands.
    ///
    /// Strict on purpose. An unreadable header means a caller's context is lost, which costs a
    /// link in a trace; a *guessed* one means two unrelated traces merge, which costs the
    /// reader their confidence in every trace. A refusal here makes [`root`](Self::root) run,
    /// and the request is traced under an id of its own.
    #[must_use]
    pub fn parse_traceparent(value: &str) -> Option<Self> {
        let value = value.trim();
        let mut parts = value.split('-');
        let version = parts.next()?;
        let trace = parts.next()?;
        let span = parts.next()?;
        let flags = parts.next()?;
        if parts.next().is_some() || version != VERSION || trace.len() != 32 || span.len() != 16 {
            return None;
        }
        let mut trace_id = [0_u8; 16];
        let mut span_id = [0_u8; 8];
        hex::decode_to_slice(trace, &mut trace_id).ok()?;
        hex::decode_to_slice(span, &mut span_id).ok()?;
        let mut flag_byte = [0_u8; 1];
        hex::decode_to_slice(flags, &mut flag_byte).ok()?;
        Self::new(trace_id, span_id, flag_byte[0] & FLAG_SAMPLED != 0)
    }
}

impl fmt::Display for TraceContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.trace_id_hex())
    }
}

/// The span field the whole workspace uses to name a trace, and the one the log store reads
/// as `correlation_id` (`rd_diagnostics::capture::CORRELATION_FIELDS`).
pub const TRACE_FIELD: &str = "trace_id";

/// Where traces go, when they go anywhere (RD-110-03).
///
/// A slice of the `service.settings` blob, so the fields are named exactly as they appear in
/// the settings document. **Off by default and off after an upgrade**: exporting traces sends
/// the shape of a person's activity to a third system, and that is a decision somebody takes
/// rather than one they discover.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct OtelSettings {
    /// Whether spans are exported at all. Everything else is inert while this is false.
    pub otlp_enabled: bool,
    /// The collector's OTLP/HTTP traces endpoint, for example
    /// `http://127.0.0.1:4318/v1/traces`. Empty means unconfigured, which is the same as off.
    pub otlp_endpoint: String,
    /// How long one export attempt may take before it is abandoned.
    pub otlp_timeout_seconds: u32,
}

impl Default for OtelSettings {
    fn default() -> Self {
        Self {
            otlp_enabled: false,
            otlp_endpoint: String::new(),
            otlp_timeout_seconds: DEFAULT_OTLP_TIMEOUT_SECONDS,
        }
    }
}

impl OtelSettings {
    /// Whether anything should be exported: switched on *and* pointed somewhere.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.otlp_enabled && !self.otlp_endpoint.trim().is_empty()
    }
}

/// Five seconds: long enough for a collector on the same host or in the same cluster, short
/// enough that an unreachable one is noticed within one batch interval.
pub const DEFAULT_OTLP_TIMEOUT_SECONDS: u32 = 5;
pub const OTLP_TIMEOUT_SECONDS_RANGE: std::ops::RangeInclusive<u32> = 1..=60;

/// Whether this is an endpoint the exporter would accept.
///
/// `http` and `https` only, and an absolute URL with a host. A `file://` or an OTLP endpoint
/// without a host would be a way to make the service open something local on a schedule.
#[must_use]
pub fn is_valid_otlp_endpoint(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return true;
    }
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https") && url.host_str().is_some_and(|host| !host.is_empty())
}

#[cfg(test)]
mod tests {
    use super::TraceContext;

    #[test]
    fn a_job_context_is_the_same_everywhere_and_differs_per_job() {
        let first = TraceContext::for_job("download", "42");
        let again = TraceContext::for_job("download", "42");
        let other = TraceContext::for_job("download", "43");
        let family = TraceContext::for_job("package", "42");
        assert_eq!(first, again);
        assert_ne!(first.trace_id(), other.trace_id());
        assert_ne!(first.trace_id(), family.trace_id());
    }

    #[test]
    fn a_traceparent_survives_being_written_and_read_back() {
        let context = TraceContext::for_job("download", "7");
        let header = context.traceparent();
        assert_eq!(header.len(), 55);
        assert_eq!(TraceContext::parse_traceparent(&header), Some(context));
    }

    #[test]
    fn an_unreadable_traceparent_is_refused_rather_than_guessed() {
        for header in [
            "",
            "00",
            "01-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01-extra",
            "00-zz-b7ad6b7169203331-01",
            "00-00000000000000000000000000000000-b7ad6b7169203331-01",
            "00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01",
        ] {
            assert_eq!(
                TraceContext::parse_traceparent(header),
                None,
                "accepted {header:?}"
            );
        }
    }

    #[test]
    fn a_child_keeps_the_trace_and_changes_the_span() {
        let parent = TraceContext::for_job("download", "9");
        let child = parent.child("resolve");
        assert_eq!(child.trace_id(), parent.trace_id());
        assert_ne!(child.span_id(), parent.span_id());
        assert_eq!(child, parent.child("resolve"));
        assert_ne!(child, parent.child("transfer"));
    }

    #[test]
    fn two_roots_never_share_an_id() {
        let first = TraceContext::root();
        let second = TraceContext::root();
        assert_ne!(first.trace_id(), second.trace_id());
        assert!(first.sampled());
    }

    #[test]
    fn the_sampled_flag_rides_in_the_header() {
        let sampled = TraceContext::for_job("download", "1");
        let off =
            TraceContext::new(sampled.trace_id(), sampled.span_id(), false).expect("non-zero ids");
        assert!(sampled.traceparent().ends_with("-01"));
        assert!(off.traceparent().ends_with("-00"));
        assert_eq!(
            TraceContext::parse_traceparent(&off.traceparent()).map(|c| c.sampled()),
            Some(false)
        );
    }

    #[test]
    fn otel_is_off_until_it_is_switched_on_and_pointed_somewhere() {
        let mut settings = super::OtelSettings::default();
        assert!(!settings.otlp_enabled);
        assert!(!settings.is_active());
        settings.otlp_enabled = true;
        assert!(
            !settings.is_active(),
            "enabled with no endpoint is still off"
        );
        settings.otlp_endpoint = "http://127.0.0.1:4318/v1/traces".to_owned();
        assert!(settings.is_active());
    }

    #[test]
    fn only_an_absolute_http_endpoint_with_a_host_is_accepted() {
        assert!(super::is_valid_otlp_endpoint(""));
        assert!(super::is_valid_otlp_endpoint(
            "http://127.0.0.1:4318/v1/traces"
        ));
        assert!(super::is_valid_otlp_endpoint(
            "https://collector.example/v1/traces"
        ));
        for bad in [
            "file:///etc/passwd",
            "/v1/traces",
            "collector:4318",
            "http://",
            "ftp://collector/v1/traces",
        ] {
            assert!(!super::is_valid_otlp_endpoint(bad), "accepted {bad}");
        }
    }
}
