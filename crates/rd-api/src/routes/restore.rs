//! Restoring a full backup (RD-160-03): uploads, preview, test restore, the restore and its
//! state. Every route costs `api:admin` (`scope_policy`).

use axum::{
    Router,
    routing::{get, post, put},
};
use utoipa::OpenApi;

use crate::{AppState, restore_handlers, restore_uploads};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/backups/restore",
            get(restore_handlers::get_restore_status)
                .post(restore_handlers::start_restore)
                .delete(restore_handlers::discard_restore),
        )
        .route(
            "/api/v1/backups/restore/preview",
            post(restore_handlers::preview_restore),
        )
        .route(
            "/api/v1/backups/restore/test",
            post(restore_handlers::test_restore),
        )
        .route(
            "/api/v1/backups/restore/uploads",
            post(restore_uploads::create_restore_upload),
        )
        .route(
            "/api/v1/backups/restore/uploads/{id}",
            put(restore_uploads::append_restore_upload)
                .delete(restore_uploads::delete_restore_upload),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    restore_handlers::get_restore_status,
    restore_handlers::start_restore,
    restore_handlers::discard_restore,
    restore_handlers::preview_restore,
    restore_handlers::test_restore,
    restore_uploads::create_restore_upload,
    restore_uploads::append_restore_upload,
    restore_uploads::delete_restore_upload,
))]
pub(crate) struct Doc;
