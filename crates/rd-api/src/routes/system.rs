//! Health, session, capture pairing, packages listing and the event stream.

use axum::{
    Router,
    routing::{delete, get, post},
};
use utoipa::OpenApi;

use crate::{AppState, about, collector_handlers, data_reset_handlers, handlers, tools_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/packages", get(handlers::list_packages))
        .route("/api/v1/system/media", get(handlers::media_status))
        .route("/api/v1/system/about", get(about::system_about))
        .route(
            "/api/v1/system/about/licenses",
            get(about::system_about_licenses),
        )
        .route(
            "/api/v1/system/data-reset",
            get(data_reset_handlers::data_reset_preview),
        )
        .route(
            "/api/v1/system/tools",
            get(tools_handlers::list_managed_tools),
        )
        .route(
            "/api/v1/system/tools/manifest/refresh",
            post(tools_handlers::refresh_tool_manifest),
        )
        .route(
            "/api/v1/system/tools/{name}/install",
            post(tools_handlers::install_managed_tool),
        )
        .route(
            "/api/v1/system/tools/{name}/activate",
            post(tools_handlers::activate_managed_tool),
        )
        .route(
            "/api/v1/system/tools/{name}/rollback",
            post(tools_handlers::rollback_managed_tool),
        )
        .route(
            "/api/v1/collector/batches",
            get(handlers::list_batches).post(collector_handlers::collector_intake),
        )
        .route(
            "/api/v1/collector/candidates",
            get(handlers::list_candidates).delete(handlers::delete_candidates),
        )
        .route(
            "/api/v1/collector/candidates/{id}",
            delete(handlers::delete_candidate).patch(collector_handlers::rename_candidate),
        )
        .route(
            "/api/v1/collector/candidates/{id}/enqueue",
            post(handlers::enqueue_candidate),
        )
        .route(
            "/api/v1/nzb/imports",
            get(handlers::list_nzb_imports).post(handlers::import_nzb),
        )
        .route(
            "/api/v1/nzb/imports/{id}",
            delete(handlers::delete_nzb_import).patch(handlers::update_nzb_import),
        )
        .route("/api/v1/capture/pair", post(handlers::pair_capture))
        .route("/api/v1/capture/agents", get(handlers::list_capture_agents))
        .route(
            "/api/v1/capture/agents/{id}",
            delete(handlers::revoke_capture_agent),
        )
        .route(
            "/api/v1/settings",
            get(handlers::get_settings).put(handlers::put_settings),
        )
        .route("/api/v1/settings/reset", post(handlers::reset_settings))
        .route("/api/v1/events", get(crate::event_stream::events))
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    handlers::health,
    handlers::auth_status,
    handlers::setup,
    handlers::login,
    handlers::pair_capture,
    handlers::capture_summary,
    handlers::list_capture_agents,
    handlers::revoke_capture_agent,
    handlers::list_packages,
    handlers::media_status,
    about::system_about,
    about::system_about_licenses,
    tools_handlers::list_managed_tools,
    tools_handlers::refresh_tool_manifest,
    tools_handlers::install_managed_tool,
    tools_handlers::activate_managed_tool,
    tools_handlers::rollback_managed_tool,
    handlers::capture_ping,
    crate::capture_file::capture_file,
    handlers::list_batches,
    handlers::list_candidates,
    handlers::delete_candidate,
    handlers::delete_candidates,
    handlers::enqueue_candidate,
    handlers::list_nzb_imports,
    handlers::import_nzb,
    handlers::update_nzb_import,
    handlers::delete_nzb_import,
    handlers::get_settings,
    handlers::put_settings,
    handlers::reset_settings,
    data_reset_handlers::data_reset_preview,
))]
pub(crate) struct Doc;
