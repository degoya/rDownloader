//! Usenet servers and NZB import routes.

use axum::{
    Router,
    routing::{get, post},
};
use utoipa::OpenApi;

use crate::{AppState, nzb_remote_job_handlers, usenet_handlers};

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
        // The other way out of the LinkGrabber for an NZB: to a provider's account rather than
        // the queue (RD-191-13).
        .route(
            "/api/v1/nzb/imports/{id}/remote-job",
            post(nzb_remote_job_handlers::submit_nzb_import_remote_job),
        )
        // The same for the NZB behind a package in the Downloads view, in any of its states.
        .route(
            "/api/v1/packages/{id}/remote-job",
            post(nzb_remote_job_handlers::submit_package_remote_job),
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
        .route(
            "/api/v1/usenet/servers/{id}/quota",
            axum::routing::put(usenet_handlers::set_usenet_server_quota),
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
    usenet_handlers::set_usenet_server_quota,
    usenet_handlers::list_nzb_files,
    usenet_handlers::list_postprocess_steps,
    usenet_handlers::enqueue_nzb_import,
    nzb_remote_job_handlers::submit_nzb_import_remote_job,
    nzb_remote_job_handlers::submit_package_remote_job,
))]
pub(crate) struct Doc;
