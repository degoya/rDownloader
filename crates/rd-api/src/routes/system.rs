//! Health, session, capture pairing, packages listing and the event stream.

use axum::{
    Router,
    routing::{delete, get, post},
};
use utoipa::OpenApi;

use crate::{
    AppState, about, about_page, candidate_handlers, capture_agent_handlers, capture_linkgrabber,
    capture_queue, capture_summary, collector_handlers, data_reset_handlers, handlers,
    login_handlers, nzb_handlers, package_handlers, settings_handlers, tools_handlers,
    update_handlers,
};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/packages", get(package_handlers::list_packages))
        .route("/api/v1/system/media", get(tools_handlers::media_status))
        .route("/api/v1/system/about", get(about_page::system_about))
        .route(
            "/api/v1/system/about/licenses",
            get(about::system_about_licenses),
        )
        .route(
            "/api/v1/system/data-reset",
            get(data_reset_handlers::data_reset_preview),
        )
        .route(
            "/api/v1/system/update",
            get(update_handlers::get_update_status),
        )
        .route(
            "/api/v1/system/update/check",
            post(update_handlers::check_for_updates),
        )
        .route(
            "/api/v1/system/update/download",
            post(update_handlers::download_update),
        )
        .route(
            "/api/v1/system/update/install",
            post(update_handlers::install_update),
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
            get(candidate_handlers::list_batches).post(collector_handlers::collector_intake),
        )
        .route(
            "/api/v1/collector/candidates",
            get(candidate_handlers::list_candidates).delete(candidate_handlers::delete_candidates),
        )
        .route(
            "/api/v1/collector/candidates/{id}",
            delete(candidate_handlers::delete_candidate)
                .patch(collector_handlers::rename_candidate),
        )
        .route(
            "/api/v1/collector/candidates/{id}/enqueue",
            post(candidate_handlers::enqueue_candidate),
        )
        .route(
            "/api/v1/nzb/imports",
            get(nzb_handlers::list_nzb_imports).post(nzb_handlers::import_nzb),
        )
        .route(
            "/api/v1/nzb/imports/{id}",
            delete(nzb_handlers::delete_nzb_import).patch(nzb_handlers::update_nzb_import),
        )
        .route("/api/v1/capture/pair", post(login_handlers::pair_capture))
        .route(
            "/api/v1/capture/agents",
            get(login_handlers::list_capture_agents),
        )
        .route(
            "/api/v1/capture/agents/{id}",
            delete(login_handlers::revoke_capture_agent),
        )
        .route(
            "/api/v1/settings",
            get(settings_handlers::get_settings).put(settings_handlers::put_settings),
        )
        .route(
            "/api/v1/settings/reset",
            post(settings_handlers::reset_settings),
        )
        .route(
            "/api/v1/settings/capture-agent",
            get(capture_agent_handlers::get_capture_agent_settings)
                .patch(capture_agent_handlers::update_capture_agent_settings),
        )
        .route("/api/v1/events", get(crate::event_stream::events))
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    handlers::health,
    login_handlers::auth_status,
    login_handlers::setup,
    login_handlers::login,
    login_handlers::pair_capture,
    capture_summary::capture_summary,
    capture_queue::pause_capture_queue,
    capture_queue::resume_capture_queue,
    capture_linkgrabber::enqueue_capture_linkgrabber,
    capture_agent_handlers::read_capture_agent_settings,
    capture_agent_handlers::set_capture_clipboard,
    capture_agent_handlers::report_capture_shortcuts,
    capture_agent_handlers::get_capture_agent_settings,
    capture_agent_handlers::update_capture_agent_settings,
    login_handlers::list_capture_agents,
    login_handlers::revoke_capture_agent,
    package_handlers::list_packages,
    tools_handlers::media_status,
    about_page::system_about,
    about::system_about_licenses,
    tools_handlers::list_managed_tools,
    tools_handlers::refresh_tool_manifest,
    tools_handlers::install_managed_tool,
    tools_handlers::activate_managed_tool,
    tools_handlers::rollback_managed_tool,
    update_handlers::get_update_status,
    update_handlers::check_for_updates,
    update_handlers::download_update,
    update_handlers::install_update,
    handlers::capture_ping,
    crate::capture_file::capture_file,
    candidate_handlers::list_batches,
    candidate_handlers::list_candidates,
    candidate_handlers::delete_candidate,
    candidate_handlers::delete_candidates,
    candidate_handlers::enqueue_candidate,
    nzb_handlers::list_nzb_imports,
    nzb_handlers::import_nzb,
    nzb_handlers::update_nzb_import,
    nzb_handlers::delete_nzb_import,
    settings_handlers::get_settings,
    settings_handlers::put_settings,
    settings_handlers::reset_settings,
    data_reset_handlers::data_reset_preview,
))]
pub(crate) struct Doc;
