//! Stopping the service and the backup before an update (RD-180-02, RD-180-03). Both cost
//! `api:admin`, are refused from anywhere but this machine, and also open to the local control
//! token (`scope_policy`, `local_control`).

use axum::{Router, routing::post};
use utoipa::OpenApi;

use crate::{AppState, lifecycle_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/system/shutdown",
            post(lifecycle_handlers::shutdown_service),
        )
        .route(
            "/api/v1/system/update/prepare",
            post(lifecycle_handlers::prepare_update_backup),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    lifecycle_handlers::shutdown_service,
    lifecycle_handlers::prepare_update_backup,
))]
pub(crate) struct Doc;
