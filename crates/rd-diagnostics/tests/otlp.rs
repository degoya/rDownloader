//! The optional trace export (RD-110-03): it is off until it is switched on, it never carries
//! a sensitive URL, and a collector that is down or hostile costs a counter and nothing else.

use std::{collections::BTreeMap, time::Duration};

use rd_core::TraceContext;
use rd_diagnostics::{
    SpanRecord, TraceExportLayer,
    otlp::{self, SERVICE_NAME},
};
use tracing::subscriber::with_default;
use tracing_subscriber::{layer::SubscriberExt, registry};

/// Values that must never leave the process in a span.
const CANARIES: &[&str] = &[
    "sk-live-4242",
    "hunter2",
    "AKIAIOSFODNN7EXAMPLE",
    "sigv4-signature-value",
    "bearer-token-value",
];

fn collect<F: FnOnce()>(emit: F) -> Vec<SpanRecord> {
    let (sink, mut stream) = otlp::channel();
    let subscriber = registry().with(TraceExportLayer::new(sink));
    otlp::set_active(true);
    with_default(subscriber, emit);
    otlp::set_active(false);
    stream.drain_ready()
}

#[tokio::test]
async fn nothing_is_built_while_the_export_is_off() {
    let (sink, mut stream) = otlp::channel();
    let subscriber = registry().with(TraceExportLayer::new(sink));
    otlp::set_active(false);
    let context = TraceContext::for_job("download", "1");
    with_default(subscriber, || {
        let span = tracing::info_span!("transfer", trace_id = %context.trace_id_hex());
        let _entered = span.enter();
    });
    assert!(
        stream.drain_ready().is_empty(),
        "a span was built although the export is off"
    );
}

#[tokio::test]
async fn a_span_with_a_trace_id_is_exported_and_its_children_join_the_same_trace() {
    let context = TraceContext::for_job("download", "77");
    let spans = collect(|| {
        let outer =
            tracing::info_span!("job", trace_id = %context.trace_id_hex(), download_id = 77);
        let _entered = outer.enter();
        let inner = tracing::info_span!("resolve", provider = "example");
        let _inner = inner.enter();
    });
    assert_eq!(spans.len(), 2, "both spans belong to the trace");
    let trace = context.trace_id_hex();
    for span in &spans {
        assert_eq!(span.trace_id, trace);
    }
    let names: Vec<&str> = spans.iter().map(|span| span.name.as_str()).collect();
    assert!(names.contains(&"job") && names.contains(&"resolve"));
    let child = spans
        .iter()
        .find(|span| span.name == "resolve")
        .expect("the inner span");
    let parent = spans
        .iter()
        .find(|span| span.name == "job")
        .expect("the outer span");
    assert_eq!(
        child.parent_span_id.as_deref(),
        Some(parent.span_id.as_str()),
        "the child names its parent"
    );
    assert!(parent.parent_span_id.is_none(), "the root has no parent");
    assert_ne!(child.span_id, parent.span_id);
}

#[tokio::test]
async fn a_span_outside_any_trace_is_not_exported() {
    let spans = collect(|| {
        let span = tracing::info_span!("unrelated", what = "work");
        let _entered = span.enter();
    });
    assert!(spans.is_empty(), "an untraced span reached the exporter");
}

#[tokio::test]
async fn no_span_attribute_carries_a_known_secret_pattern() {
    let context = TraceContext::for_job("download", "3");
    let spans = collect(|| {
        let span = tracing::info_span!(
            "transfer",
            trace_id = %context.trace_id_hex(),
            url = "https://cdn.example/file.bin?token=sk-live-4242&X-Amz-Signature=sigv4-signature-value",
            password = "hunter2",
            authorization = "Bearer bearer-token-value",
            api_key = "AKIAIOSFODNN7EXAMPLE",
        );
        let _entered = span.enter();
    });
    let encoded = serde_json::to_string(&otlp::encode(&spans, "1.1.0")).expect("json");
    for canary in CANARIES {
        assert!(
            !encoded.contains(canary),
            "the span export carried {canary}: {encoded}"
        );
    }
    assert!(encoded.contains("redacted"), "nothing was redacted at all");
}

#[tokio::test]
async fn the_encoding_is_the_document_a_collector_expects() {
    let span = SpanRecord {
        trace_id: "0af7651916cd43dd8448eb211c80319c".to_owned(),
        span_id: "b7ad6b7169203331".to_owned(),
        parent_span_id: None,
        name: "http.request".to_owned(),
        start_unix_nano: 1_700_000_000_000_000_000,
        end_unix_nano: 1_700_000_000_500_000_000,
        failed: false,
        attributes: BTreeMap::from([("http.method".to_owned(), "GET".to_owned())]),
    };
    let document = otlp::encode(&[span], "1.1.0");
    let resource = &document["resourceSpans"][0];
    assert_eq!(resource["resource"]["attributes"][0]["key"], "service.name");
    assert_eq!(
        resource["resource"]["attributes"][0]["value"]["stringValue"],
        SERVICE_NAME
    );
    let encoded = &resource["scopeSpans"][0]["spans"][0];
    assert_eq!(encoded["traceId"], "0af7651916cd43dd8448eb211c80319c");
    assert_eq!(encoded["spanId"], "b7ad6b7169203331");
    assert_eq!(encoded["kind"], 1);
    // Proto3 JSON maps 64-bit integers to strings; a number would lose nanoseconds.
    assert_eq!(encoded["startTimeUnixNano"], "1700000000000000000");
    assert_eq!(encoded["endTimeUnixNano"], "1700000000500000000");
    assert_eq!(encoded["attributes"][0]["key"], "http.method");
    assert_eq!(encoded["attributes"][0]["value"]["stringValue"], "GET");
}

#[tokio::test]
async fn an_unreachable_collector_fails_fast_and_takes_nothing_with_it() {
    // A port nothing listens on: bound, read, and released before the export runs.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("address").port();
    drop(listener);

    let client = reqwest::Client::builder().build().expect("client");
    let span = SpanRecord {
        trace_id: "a".repeat(32),
        span_id: "b".repeat(16),
        name: "transfer".to_owned(),
        ..SpanRecord::default()
    };
    let started = std::time::Instant::now();
    let result = otlp::export(
        &client,
        &format!("http://127.0.0.1:{port}/v1/traces"),
        Duration::from_secs(2),
        std::slice::from_ref(&span),
        "1.1.0",
    )
    .await;
    assert!(
        result.is_err(),
        "an unreachable collector must report an error"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the export did not give up within its timeout"
    );

    // And the work goes on: the layer still accepts spans afterwards, and the counters say
    // what happened rather than the process stopping.
    otlp::note_export(1, false);
    let after = otlp::snapshot();
    assert!(after.failed >= 1);
    let spans = collect(|| {
        let context = TraceContext::for_job("download", "after");
        let span = tracing::info_span!("still.working", trace_id = %context.trace_id_hex());
        let _entered = span.enter();
    });
    assert_eq!(spans.len(), 1, "logging stopped after an export failure");
}

#[tokio::test]
async fn a_collector_that_answers_with_an_error_is_not_retried() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("address").port();
    let served = tokio::spawn(async move {
        let mut seen = 0usize;
        while let Ok((mut stream, _)) = listener.accept().await {
            seen += 1;
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buffer = [0_u8; 4096];
            let _read = stream.read(&mut buffer).await;
            let _written = stream
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _flushed = stream.flush().await;
            if seen >= 1 {
                return seen;
            }
        }
        seen
    });

    let client = reqwest::Client::builder().build().expect("client");
    let span = SpanRecord {
        trace_id: "a".repeat(32),
        span_id: "b".repeat(16),
        name: "transfer".to_owned(),
        ..SpanRecord::default()
    };
    let result = otlp::export(
        &client,
        &format!("http://127.0.0.1:{port}/v1/traces"),
        Duration::from_secs(5),
        std::slice::from_ref(&span),
        "1.1.0",
    )
    .await;
    let error = result.expect_err("a 500 must be reported");
    assert!(
        error.to_string().contains("500"),
        "unexpected error: {error}"
    );
    let requests = served.await.expect("server task");
    assert_eq!(requests, 1, "the batch was sent more than once");
}
