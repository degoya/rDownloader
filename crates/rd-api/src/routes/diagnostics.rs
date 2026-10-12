//! The log viewer and the diagnostic bundle (RD-110-02); the log level and a debug window
//! (RD-1260-12).

use axum::{
    Router,
    routing::{get, post, put},
};
use utoipa::OpenApi;

use crate::{AppState, data_reset_handlers, diagnostics_handlers, log_level_handlers};

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
            "/api/v1/diagnostics/log-level",
            get(log_level_handlers::get_log_level).put(log_level_handlers::set_log_level),
        )
        .route(
            "/api/v1/diagnostics/log-level/debug",
            put(log_level_handlers::start_log_debug).delete(log_level_handlers::stop_log_debug),
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
    log_level_handlers::get_log_level,
    log_level_handlers::set_log_level,
    log_level_handlers::start_log_debug,
    log_level_handlers::stop_log_debug,
    diagnostics_handlers::preview_diagnostic_bundle,
    diagnostics_handlers::create_diagnostic_bundle,
    diagnostics_handlers::download_diagnostic_bundle,
))]
pub(crate) struct Doc;
