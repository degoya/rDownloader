//! The secret canaries (RD-110-02): every shape a credential arrives in is logged through the
//! capture layer, and none of it survives into the record the store would receive.

use std::sync::Arc;

use rd_diagnostics::capture::{CaptureStats, LogCaptureLayer};
use tracing::subscriber::with_default;
use tracing_subscriber::{layer::SubscriberExt, registry};

/// Values that must never appear in a stored record.
const CANARIES: &[&str] = &[
    "sk-live-4242",
    "hunter2",
    "AKIAIOSFODNN7EXAMPLE",
    "sigv4-signature-value",
    "session-cookie-value",
    "bearer-token-value",
    "vault-reference-name",
    "azure-sas-sig",
];

fn capture<F: FnOnce()>(emit: F) -> Vec<rd_db::NewLogRecord> {
    let (layer, mut stream) = LogCaptureLayer::with_stats(Arc::new(CaptureStats::default()));
    let subscriber = registry().with(layer);
    with_default(subscriber, emit);
    stream.drain_ready()
}

fn assert_clean(records: &[rd_db::NewLogRecord]) {
    let rendered = serde_json::to_string(records).expect("json");
    for canary in CANARIES {
        assert!(
            !rendered.contains(canary),
            "canary {canary:?} survived into a record: {rendered}"
        );
    }
}

#[test]
fn a_url_with_a_credential_in_its_query_keeps_the_name_and_loses_the_value() {
    let records = capture(|| {
        tracing::warn!(
            url = "https://cdn.example.com/f.bin?token=sk-live-4242&X-Amz-Signature=sigv4-signature-value&X-Amz-Expires=300",
            "download failed for https://host.example/x?api_key=sk-live-4242"
        );
    });
    assert_eq!(records.len(), 1);
    assert_clean(&records);
    let url = records[0].fields.get("url").expect("url field");
    // A query value is written back through the URL serializer, so the placeholder arrives
    // percent-encoded there and bare everywhere else; both are the central redaction's forms.
    assert!(url.contains("token=%5Bredacted%5D"), "{url}");
    assert!(
        url.contains("X-Amz-Expires=300"),
        "expiry is not a secret: {url}"
    );
    assert!(
        records[0].message.contains("api_key=%5Bredacted%5D"),
        "{}",
        records[0].message
    );
}

#[test]
fn a_password_in_a_transfer_url_and_a_bare_field_are_both_replaced() {
    let records = capture(|| {
        tracing::error!(
            password = "hunter2",
            source = "ftp://alice:hunter2@files.example.net/pub/",
            "login refused"
        );
    });
    assert_clean(&records);
    assert_eq!(
        records[0].fields.get("password").map(String::as_str),
        Some("[redacted]")
    );
    let source = records[0].fields.get("source").expect("source");
    assert_eq!(
        source, "ftp://redacted@files.example.net/pub/",
        "userinfo goes as a whole; the host and path stay"
    );
}

#[test]
fn header_values_bearer_schemes_and_vault_references_are_replaced() {
    let records = capture(|| {
        tracing::info!(
            authorization = "Bearer bearer-token-value",
            cookie = "session=session-cookie-value",
            api_key = "AKIAIOSFODNN7EXAMPLE",
            secret_ref = "vault://vault-reference-name",
            "Authorization: Bearer bearer-token-value was refused; retry with basic hunter2"
        );
    });
    assert_clean(&records);
    let fields = &records[0].fields;
    assert_eq!(
        fields.get("authorization").map(String::as_str),
        Some("[redacted]")
    );
    assert_eq!(fields.get("cookie").map(String::as_str), Some("[redacted]"));
    assert_eq!(
        fields.get("api_key").map(String::as_str),
        Some("[redacted]")
    );
    assert_eq!(
        fields.get("secret_ref").map(String::as_str),
        Some("vault://[redacted]"),
        "a vault reference keeps its scheme so the reader knows what kind of value stood there"
    );
}

#[test]
fn signed_url_parameters_are_replaced_whatever_provider_signed_them() {
    let records = capture(|| {
        tracing::warn!(
            "expired: https://blob.example/c/f?sig=azure-sas-sig&se=2026-01-01 and https://s3.example/k?X-Amz-Credential=AKIAIOSFODNN7EXAMPLE"
        );
    });
    assert_clean(&records);
    assert!(
        records[0].message.contains("se=2026-01-01"),
        "{}",
        records[0].message
    );
}

#[test]
fn the_record_carries_level_component_code_and_the_correlation_of_its_span() {
    let records = capture(|| {
        let span = tracing::info_span!("transfer", download_id = "dl-42", token = "sk-live-4242");
        let _entered = span.enter();
        tracing::warn!(code = "http.status", status = 503, "upstream unavailable");
    });
    assert_eq!(records.len(), 1);
    assert_clean(&records);
    let record = &records[0];
    assert_eq!(record.level, rd_core::LogLevel::Warn);
    assert_eq!(record.component, "redaction");
    assert_eq!(record.code.as_deref(), Some("http.status"));
    assert_eq!(record.correlation_id.as_deref(), Some("dl-42"));
    assert_eq!(record.fields.get("status").map(String::as_str), Some("503"));
    assert_eq!(
        record.fields.get("download_id").map(String::as_str),
        Some("dl-42")
    );
    assert!(
        !record.fields.contains_key("code"),
        "the code is a column, not a field"
    );
    assert!(
        !record.fields.contains_key("token"),
        "a span field that is not a correlation is not copied: {:?}",
        record.fields
    );
}

#[test]
fn an_event_field_outranks_the_span_and_the_most_specific_name_wins() {
    let records = capture(|| {
        let outer = tracing::info_span!("package", package_id = "pkg-1");
        let _outer = outer.enter();
        let inner = tracing::info_span!("download", download_id = "dl-1");
        let _inner = inner.enter();
        tracing::info!("inherited");
        tracing::info!(correlation_id = "req-9", "explicit");
    });
    assert_eq!(
        records[0].correlation_id.as_deref(),
        Some("dl-1"),
        "innermost span first"
    );
    assert_eq!(
        records[0].fields.get("package_id").map(String::as_str),
        Some("pkg-1")
    );
    assert_eq!(records[1].correlation_id.as_deref(), Some("req-9"));
}

#[test]
fn trace_events_are_not_stored_and_a_full_channel_drops_instead_of_blocking() {
    let stats = Arc::new(CaptureStats::default());
    let (layer, mut stream) = LogCaptureLayer::with_stats(Arc::clone(&stats));
    let subscriber = registry().with(layer);
    let capacity = rd_diagnostics::capture::CHANNEL_CAPACITY;
    // Returns: a logger that blocked on a full channel would never get past the loop.
    with_default(subscriber, || {
        tracing::trace!("never stored");
        for index in 0..(capacity + 10) {
            tracing::info!(index, "burst");
        }
    });
    let snapshot = stats.snapshot();
    assert_eq!(snapshot.captured, capacity as u64);
    assert_eq!(snapshot.dropped, 10);
    let records = stream.drain_ready();
    assert_eq!(records.len(), capacity);
    assert!(records.iter().all(|record| record.message == "burst"));
}

#[test]
fn an_oversized_value_is_cut_rather_than_stored_whole() {
    let records = capture(|| {
        let excerpt = "x".repeat(10_000);
        tracing::warn!(excerpt, "page did not parse");
    });
    let stored = records[0].fields.get("excerpt").expect("excerpt");
    assert!(stored.len() <= rd_diagnostics::capture::MAX_VALUE_LENGTH + 3);
    assert!(stored.ends_with("..."));
}
