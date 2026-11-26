//! The endpoints the router answers without an area of their own: health, the OpenAPI
//! document and the capture agent's ping.

use std::sync::OnceLock;

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use rd_api_core::extract::Json;
use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::AppState;

/// Answers 503 with `status: "unavailable"` once the database writer is gone: the process
/// still serves reads, but nothing it does is persisted any more, and a supervisor or a
/// readiness probe should see that rather than an `ok`.
#[utoipa::path(get, path = "/api/v1/health", tag = "system", responses((status = 200, description = "The service works", body = HealthResponse), (status = 503, description = "The database writer is gone", body = HealthResponse)))]
pub(crate) async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    let (code, status) = if state.database.is_writable() {
        (StatusCode::OK, "ok")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable")
    };
    (
        code,
        Json(HealthResponse {
            status: status.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            service: "rDownloader".to_owned(),
        }),
    )
}

/// Whether the service works, and which version it is.
#[derive(Serialize, ToSchema)]
pub(crate) struct HealthResponse {
    /// `ok`, or `unavailable` once nothing can be stored any more.
    status: String,
    /// The service's version, `1.48.0`.
    version: String,
    /// Always `rDownloader`.
    service: String,
}

/// The document and its tag. It describes the binary, so it is built once per process: each
/// request used to derive and serialise all of it again, 40 to 50 ms for anyone who asks, as the
/// route needs no sign-in (RD-1480-19).
static OPENAPI: OnceLock<(Bytes, String)> = OnceLock::new();

pub(crate) async fn openapi(headers: HeaderMap) -> Result<Response, crate::ApiError> {
    let (document, etag) = match OPENAPI.get() {
        Some(built) => built,
        None => {
            let document = serde_json::to_vec(&crate::openapi_document())
                .map_err(|error| anyhow::anyhow!("serialise the OpenAPI document: {error}"))?;
            let etag = format!("\"{}\"", hex::encode(&Sha256::digest(&document)[..16]));
            OPENAPI.get_or_init(|| (Bytes::from(document), etag))
        }
    };
    let mut caching: Vec<(HeaderName, HeaderValue)> =
        vec![(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))];
    if let Ok(value) = HeaderValue::from_str(etag) {
        caching.push((header::ETAG, value));
    }
    let caching = HeaderMap::from_iter(caching);
    if crate::conditional::has_current(&headers, etag) {
        return Ok((StatusCode::NOT_MODIFIED, caching).into_response());
    }
    let json = [(header::CONTENT_TYPE, "application/json")];
    Ok((caching, json, document.clone()).into_response())
}

/// Token check for external clients (browser extension "test connection").
/// `capture_version` tells clients whether structured links with request metadata
/// are accepted; older servers omit it and only understand the text payload.
#[utoipa::path(get, path = "/api/v1/capture/ping", tag = "capture", responses((status = 200, body = Object), (status = 401)))]
pub(crate) async fn capture_ping() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "service": "rDownloader",
        "capture_version": rd_core::CAPTURE_CONTRACT_VERSION
    }))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::to_bytes,
        http::{HeaderMap, StatusCode, header},
    };

    use super::openapi;

    #[tokio::test]
    async fn the_document_is_the_generated_one_and_confirmed_by_its_tag() {
        let full = openapi(HeaderMap::new())
            .await
            .unwrap_or_else(|_| panic!("the document is built"));
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::CONTENT_TYPE], "application/json");
        let etag = full.headers()[header::ETAG].clone();
        let body = to_bytes(full.into_body(), usize::MAX).await.expect("body");
        let served: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(
            served,
            serde_json::to_value(crate::openapi_document()).expect("serialise")
        );

        let again = openapi(HeaderMap::from_iter([(
            header::IF_NONE_MATCH,
            etag.clone(),
        )]))
        .await
        .unwrap_or_else(|_| panic!("the document is built"));
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(again.headers()[header::ETAG], etag);
        let body = to_bytes(again.into_body(), usize::MAX).await.expect("body");
        assert!(body.is_empty());
    }
}
