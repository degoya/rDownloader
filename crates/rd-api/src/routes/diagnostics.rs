//! The log viewer and the diagnostic bundle (RD-110-02).

use axum::{
    Router,
    routing::{get, post},
};
use utoipa::OpenApi;

use crate::{AppState, data_reset_handlers, diagnostics_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/diagnostics/logs",
            get(diagnostics_handlers::list_log_records),
        )
        .route(
            "/api/v1/diagnostics/logs/clear",
            post(data_reset_handlers::clear_log_records),
        )
        .route(
            "/api/v1/diagnostics/bundle/preview",
            get(diagnostics_handlers::preview_diagnostic_bundle),
        )
        .route(
            "/api/v1/diagnostics/bundle",
            post(diagnostics_handlers::create_diagnostic_bundle),
        )
        .route(
            "/api/v1/diagnostics/bundles/{name}",
            get(diagnostics_handlers::download_diagnostic_bundle),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    diagnostics_handlers::list_log_records,
    data_reset_handlers::clear_log_records,
    diagnostics_handlers::preview_diagnostic_bundle,
    diagnostics_handlers::create_diagnostic_bundle,
    diagnostics_handlers::download_diagnostic_bundle,
))]
pub(crate) struct Doc;
