//! The download history (RD-1100-04): the list, "add again", the clear and the export
//! (RD-1240-14).

use axum::{
    Router,
    routing::{get, post},
};
use rd_api_intake::history_readd_handlers;
use rd_api_queue::{history_export, history_handlers};
use utoipa::OpenApi;

use crate::{AppState, data_reset_handlers};

/// Session- or token-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/history",
            get(history_handlers::list_download_history),
        )
        .route(
            "/api/v1/history/export",
            get(history_export::export_download_history),
        )
        .route(
            "/api/v1/history/clear",
            post(data_reset_handlers::clear_download_history),
        )
        .route(
            "/api/v1/history/{id}/readd",
            post(history_readd_handlers::readd_history_entry),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    history_handlers::list_download_history,
    history_export::export_download_history,
    data_reset_handlers::clear_download_history,
    history_readd_handlers::readd_history_entry,
))]
pub(crate) struct Doc;
