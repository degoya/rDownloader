//! The endpoints the router answers without an area of their own: health, the OpenAPI
//! document and the capture agent's ping.

use axum::Json;

#[utoipa::path(get, path = "/api/v1/health", tag = "system", responses((status = 200, body = Object)))]
pub(crate) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "rDownloader"
    }))
}

pub(crate) async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(crate::openapi_document())
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
