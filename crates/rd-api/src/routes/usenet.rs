//! Usenet servers and NZB import routes.

use axum::{
    Router,
    routing::{get, post},
};
use utoipa::OpenApi;

use crate::{AppState, usenet_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/nzb/imports/{id}/files",
            get(usenet_handlers::list_nzb_files),
        )
        .route(
            "/api/v1/nzb/imports/{id}/postprocess",
            get(usenet_handlers::list_postprocess_steps),
        )
        .route(
            "/api/v1/nzb/imports/{id}/enqueue",
            post(usenet_handlers::enqueue_nzb_import),
        )
        .route(
            "/api/v1/usenet/servers",
            get(usenet_handlers::list_usenet_servers).post(usenet_handlers::create_usenet_server),
        )
        .route(
            "/api/v1/usenet/servers/{id}",
            axum::routing::put(usenet_handlers::update_usenet_server)
                .delete(usenet_handlers::delete_usenet_server),
        )
        .route(
            "/api/v1/usenet/servers/{id}/test",
            post(usenet_handlers::test_usenet_server),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    usenet_handlers::list_usenet_servers,
    usenet_handlers::create_usenet_server,
    usenet_handlers::update_usenet_server,
    usenet_handlers::delete_usenet_server,
    usenet_handlers::test_usenet_server,
    usenet_handlers::list_nzb_files,
    usenet_handlers::list_postprocess_steps,
    usenet_handlers::enqueue_nzb_import,
))]
pub(crate) struct Doc;
