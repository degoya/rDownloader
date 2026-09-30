//! Collision policies and prompts, duplicates, dedupe links and the storage history
//! (RD-150-01, RD-150-02). The two clears are `data_reset_handlers` beside the other clears
//! (RD-180-13).

use axum::{
    Router,
    routing::{get, post, put},
};
use utoipa::OpenApi;

use crate::{AppState, collision_handlers, data_reset_handlers, duplicates, storage_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/collision-policies",
            get(collision_handlers::list_collision_policies),
        )
        .route(
            "/api/v1/categories/{id}/collision-policy",
            put(collision_handlers::set_category_collision_policy),
        )
        .route(
            "/api/v1/packages/{id}/collision-policy",
            get(collision_handlers::get_package_collision_policy)
                .put(collision_handlers::set_package_collision_policy),
        )
        .route(
            "/api/v1/collision-prompts",
            get(collision_handlers::list_collision_prompts),
        )
        .route(
            "/api/v1/downloads/{id}/collision-decision",
            post(collision_handlers::decide_collision),
        )
        .route(
            "/api/v1/downloads/{id}/duplicates",
            get(duplicates::download_duplicates),
        )
        .route(
            "/api/v1/duplicates/lookup",
            post(duplicates::lookup_duplicates),
        )
        .route(
            "/api/v1/downloads/{id}/dedupe",
            post(storage_handlers::dedupe_download),
        )
        .route(
            "/api/v1/storage/reuse",
            get(storage_handlers::reuse_capabilities),
        )
        .route(
            "/api/v1/storage/link-support",
            get(storage_handlers::link_support),
        )
        .route(
            "/api/v1/storage/operations",
            get(storage_handlers::list_storage_operations),
        )
        .route(
            "/api/v1/storage/operations/clear",
            post(data_reset_handlers::clear_storage_operations),
        )
        .route(
            "/api/v1/storage/content-index/check",
            post(storage_handlers::check_content_index),
        )
        .route(
            "/api/v1/storage/content-index/clear",
            post(data_reset_handlers::clear_content_index),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    collision_handlers::list_collision_policies,
    collision_handlers::set_category_collision_policy,
    collision_handlers::get_package_collision_policy,
    collision_handlers::set_package_collision_policy,
    collision_handlers::list_collision_prompts,
    collision_handlers::decide_collision,
    duplicates::download_duplicates,
    duplicates::lookup_duplicates,
    storage_handlers::dedupe_download,
    storage_handlers::reuse_capabilities,
    storage_handlers::link_support,
    storage_handlers::list_storage_operations,
    data_reset_handlers::clear_storage_operations,
    storage_handlers::check_content_index,
    data_reset_handlers::clear_content_index,
))]
pub(crate) struct Doc;
