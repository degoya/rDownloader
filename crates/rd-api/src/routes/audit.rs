//! The audit log (RD-110-03). Two reads, and one clear that writes itself into the emptied
//! log as its first entry (RD-120-34). There is still no route that edits or removes a single
//! record: migration `0079` refuses an UPDATE, and a clear that could pick its rows would be
//! a way to rewrite history rather than to start a test run.

use axum::{
    Router,
    routing::{get, post},
};
use utoipa::OpenApi;

use crate::{AppState, audit_handlers, data_reset_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/audit/records",
            get(audit_handlers::list_audit_records),
        )
        .route(
            "/api/v1/audit/records/clear",
            post(data_reset_handlers::clear_audit_records),
        )
        .route(
            "/api/v1/audit/export",
            get(audit_handlers::export_audit_records),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    audit_handlers::list_audit_records,
    audit_handlers::export_audit_records,
    data_reset_handlers::clear_audit_records,
))]
pub(crate) struct Doc;
