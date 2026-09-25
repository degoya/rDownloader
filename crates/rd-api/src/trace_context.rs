//! The trace a request belongs to (RD-110-03).
//!
//! The middleware does three things and nothing else:
//!
//! 1. Takes the caller's `traceparent` when it sent a readable one, and makes a root when it
//!    did not. A distributed trace that already started behind a reverse proxy or in a client
//!    continues here rather than beginning again.
//! 2. Puts the context on the request, so a handler writing an audit record can say which
//!    trace the action belonged to without knowing anything about headers.
//! 3. Opens a span carrying `trace_id` and enters it for the rest of the request. That one
//!    field is what ties the whole request together: `rd_diagnostics::capture` copies it onto
//!    every log record written inside, and `rd_diagnostics::trace_layer` exports every span
//!    opened inside as part of the same trace. Nothing else has to be passed anywhere.
//!
//! The response carries the context back as `traceparent`, so a caller that started the trace
//! can see which span answered it, and one that did not can pick the id out of a failed
//! request and paste it into the log viewer.
//!
//! **No URL and no query string goes on the span.** The method and the *matched route
//! pattern* do — `/api/v1/downloads/{id}`, never `/api/v1/downloads/42?token=...`. A path
//! parameter is a download id at worst; a raw URI is where a signed link would be.

use axum::{
    extract::{MatchedPath, Request},
    http::{HeaderValue, header::HeaderName},
    middleware::Next,
    response::Response,
};
use rd_core::TraceContext;
use tracing::Instrument;

/// The W3C header this service reads and writes.
pub(crate) const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");

/// Establishes the trace context for one request. See the module documentation.
pub(crate) async fn attach(mut request: Request, next: Next) -> Response {
    let context = request
        .headers()
        .get(TRACEPARENT)
        .and_then(|value| value.to_str().ok())
        .and_then(TraceContext::parse_traceparent)
        .unwrap_or_else(TraceContext::root);
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned());
    let method = request.method().clone();
    request.extensions_mut().insert(context);

    let span = tracing::info_span!(
        "http.request",
        trace_id = %context.trace_id_hex(),
        // `http.request.method` and `http.route` are the OpenTelemetry semantic-convention
        // names, so a collector groups these without a mapping rule.
        "http.request.method" = %method,
        "http.route" = route.as_deref().unwrap_or("unmatched"),
    );
    let mut response = next.run(request).instrument(span).await;
    if let Ok(value) = HeaderValue::from_str(&context.traceparent()) {
        response.headers_mut().insert(TRACEPARENT, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, http::Request, routing::get};
    use rd_core::TraceContext;
    use tower::ServiceExt;

    use super::{TRACEPARENT, attach};

    fn app() -> Router {
        Router::new()
            .route(
                "/probe",
                get(
                    |extension: axum::extract::Extension<TraceContext>| async move {
                        extension.0.trace_id_hex()
                    },
                ),
            )
            .layer(axum::middleware::from_fn(attach))
    }

    async fn body_of(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1_024)
            .await
            .expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf-8")
    }

    #[tokio::test]
    async fn a_callers_trace_is_continued_rather_than_restarted() {
        let caller = TraceContext::for_job("download", "upstream");
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(TRACEPARENT, caller.traceparent())
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let echoed = response
            .headers()
            .get(TRACEPARENT)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        assert_eq!(echoed.as_deref(), Some(caller.traceparent().as_str()));
        assert_eq!(body_of(response).await, caller.trace_id_hex());
    }

    #[tokio::test]
    async fn a_request_without_a_traceparent_gets_one_of_its_own() {
        let first = app()
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let first = body_of(first).await;
        let second = app()
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let second = body_of(second).await;
        assert_eq!(first.len(), 32);
        assert_ne!(first, second, "two requests shared a trace id");
    }

    #[tokio::test]
    async fn an_unreadable_traceparent_does_not_poison_the_trace() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/probe")
                    .header(TRACEPARENT, "not-a-traceparent")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(body_of(response).await.len(), 32);
    }
}
