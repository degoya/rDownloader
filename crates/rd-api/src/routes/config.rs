//! Categories, storage roots, hotfolders, accounts, proxies, settings and setup routes.

use axum::{
    Router,
    routing::{get, post},
};
use utoipa::OpenApi;

use crate::{
    AppState, auth_flow_handlers, browser_session_handlers, config_handlers, hosters,
    providers_handlers, regex_tester, remote_job_handlers, routing_backup, settings_backup,
    setup_handlers, storage_capacity,
};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/accounts/{id}/hosters",
            get(hosters::list_account_hosters),
        )
        .route(
            "/api/v1/storage-roots",
            get(config_handlers::list_storage_roots).post(config_handlers::create_storage_root),
        )
        .route(
            "/api/v1/categories",
            get(config_handlers::list_categories).post(config_handlers::create_category),
        )
        .route(
            "/api/v1/category-rules",
            get(config_handlers::list_category_rules).post(config_handlers::create_category_rule),
        )
        .route(
            "/api/v1/hotfolders",
            get(config_handlers::list_hotfolders).post(config_handlers::create_hotfolder),
        )
        .route(
            "/api/v1/storage-roots/{id}",
            axum::routing::put(config_handlers::update_storage_root)
                .delete(config_handlers::delete_storage_root),
        )
        .route(
            "/api/v1/categories/{id}",
            axum::routing::put(config_handlers::update_category)
                .delete(config_handlers::delete_category),
        )
        .route(
            "/api/v1/category-rules/{id}",
            axum::routing::put(config_handlers::update_category_rule)
                .delete(config_handlers::delete_category_rule),
        )
        .route(
            "/api/v1/category-rules/test-regex",
            axum::routing::post(regex_tester::test_category_rule_regex),
        )
        .route(
            "/api/v1/hotfolders/{id}",
            axum::routing::put(config_handlers::update_hotfolder)
                .delete(config_handlers::delete_hotfolder),
        )
        .route(
            "/api/v1/accounts",
            get(config_handlers::list_accounts).post(config_handlers::create_account),
        )
        .route(
            "/api/v1/accounts/{id}",
            axum::routing::put(config_handlers::update_account)
                .delete(config_handlers::delete_account),
        )
        .route(
            "/api/v1/accounts/{id}/test",
            post(config_handlers::test_account),
        )
        .route(
            "/api/v1/accounts/{id}/browser-session",
            get(browser_session_handlers::get_browser_session)
                .post(browser_session_handlers::begin_browser_session)
                .delete(browser_session_handlers::cancel_browser_session),
        )
        .route(
            "/api/v1/proxy-profiles",
            get(config_handlers::list_proxy_profiles).post(config_handlers::create_proxy_profile),
        )
        .route(
            "/api/v1/proxy-profiles/{id}",
            axum::routing::put(config_handlers::update_proxy_profile)
                .delete(config_handlers::delete_proxy_profile),
        )
        .route(
            "/api/v1/accounts/{id}/auth/begin",
            post(auth_flow_handlers::begin_auth),
        )
        .route(
            "/api/v1/accounts/{id}/auth",
            get(auth_flow_handlers::get_auth).delete(auth_flow_handlers::cancel_auth),
        )
        .route(
            "/api/v1/oauth/callback",
            get(auth_flow_handlers::oauth_callback),
        )
        // Jobs that run at a provider (RD-108-04). Deleting at the provider and removing the
        // row from this list are separate paths on purpose: the first is a POST that has to
        // carry a confirmation, the second a plain DELETE that sends nothing anywhere.
        .route(
            "/api/v1/remote-jobs",
            get(remote_job_handlers::list_remote_jobs),
        )
        // Which providers can take a job at all, read from the installed manifests. A static
        // segment beside `{id}`: `providers` is not a job identifier and never will be.
        .route(
            "/api/v1/remote-jobs/providers",
            get(remote_job_handlers::list_remote_job_providers),
        )
        .route(
            "/api/v1/remote-jobs/{id}",
            axum::routing::delete(remote_job_handlers::forget_remote_job),
        )
        .route(
            "/api/v1/remote-jobs/{id}/choice",
            post(remote_job_handlers::choose_remote_job_entries),
        )
        .route(
            "/api/v1/remote-jobs/{id}/discard",
            post(remote_job_handlers::discard_remote_job),
        )
        .route(
            "/api/v1/accounts/{id}/remote-jobs",
            post(remote_job_handlers::submit_remote_job),
        )
        .route("/api/v1/providers", get(providers_handlers::list_providers))
        .route("/api/v1/setup/status", get(setup_handlers::setup_status))
        .route(
            "/api/v1/setup/complete",
            post(setup_handlers::complete_setup),
        )
        .route(
            "/api/v1/storage/capacity",
            get(storage_capacity::storage_capacity),
        )
        .route(
            "/api/v1/storage/capacity/{target}/resume",
            post(storage_capacity::resume_storage),
        )
        .route(
            "/api/v1/routing/export",
            get(routing_backup::export_routing),
        )
        .route(
            "/api/v1/routing/import",
            post(routing_backup::import_routing),
        )
        .route(
            "/api/v1/settings/export",
            post(settings_backup::export_settings),
        )
        .route(
            "/api/v1/settings/import",
            post(settings_backup::import_settings),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    setup_handlers::setup_status,
    setup_handlers::complete_setup,
    settings_backup::export_settings,
    settings_backup::import_settings,
    routing_backup::export_routing,
    routing_backup::import_routing,
    storage_capacity::storage_capacity,
    storage_capacity::resume_storage,
    config_handlers::list_storage_roots,
    config_handlers::create_storage_root,
    config_handlers::update_storage_root,
    config_handlers::delete_storage_root,
    config_handlers::list_categories,
    config_handlers::create_category,
    config_handlers::update_category,
    config_handlers::delete_category,
    config_handlers::list_category_rules,
    config_handlers::create_category_rule,
    config_handlers::update_category_rule,
    config_handlers::delete_category_rule,
    regex_tester::test_category_rule_regex,
    config_handlers::list_hotfolders,
    config_handlers::create_hotfolder,
    config_handlers::update_hotfolder,
    config_handlers::delete_hotfolder,
    config_handlers::list_accounts,
    config_handlers::create_account,
    config_handlers::update_account,
    config_handlers::delete_account,
    config_handlers::test_account,
    browser_session_handlers::begin_browser_session,
    browser_session_handlers::get_browser_session,
    browser_session_handlers::cancel_browser_session,
    // Registered in the capture router (see `crate::router`), documented here beside the
    // account half of the same handover (RD-120-45).
    browser_session_handlers::list_capture_browser_sessions,
    browser_session_handlers::deliver_capture_browser_session,
    browser_session_handlers::decline_capture_browser_session,
    hosters::list_account_hosters,
    config_handlers::list_proxy_profiles,
    config_handlers::create_proxy_profile,
    config_handlers::update_proxy_profile,
    config_handlers::delete_proxy_profile,
    auth_flow_handlers::begin_auth,
    auth_flow_handlers::get_auth,
    auth_flow_handlers::cancel_auth,
    auth_flow_handlers::oauth_callback,
    providers_handlers::list_providers,
    remote_job_handlers::list_remote_jobs,
    remote_job_handlers::list_remote_job_providers,
    remote_job_handlers::submit_remote_job,
    remote_job_handlers::choose_remote_job_entries,
    remote_job_handlers::discard_remote_job,
    remote_job_handlers::forget_remote_job,
))]
pub(crate) struct Doc;
