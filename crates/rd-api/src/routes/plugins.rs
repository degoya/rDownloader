//! Plugin inventory, installation, diagnostics, trusted-key and repository routes.

use axum::{
    Router,
    routing::{delete, get, patch, post, put},
};
use utoipa::OpenApi;

use crate::{
    AppState, plugin_bundled, plugin_handlers, plugin_lifecycle, plugin_repository_handlers,
    plugin_update_policy,
};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/plugins", get(plugin_handlers::list_plugins))
        .route(
            "/api/v1/plugins/{id}/executions",
            get(plugin_handlers::list_plugin_executions),
        )
        .route(
            "/api/v1/plugins/{id}/{version}",
            axum::routing::delete(plugin_handlers::remove_plugin_version),
        )
        .route(
            "/api/v1/plugins/{id}",
            axum::routing::patch(plugin_handlers::set_plugin_enabled),
        )
        .route(
            "/api/v1/plugins/install",
            post(plugin_handlers::install_plugin),
        )
        // One segment deeper than `/{id}/{version}`, so no version string can reach these.
        .route(
            "/api/v1/plugins/{id}/lifecycle/activate",
            post(plugin_lifecycle::activate_plugin_version),
        )
        .route(
            "/api/v1/plugins/{id}/lifecycle/stage",
            post(plugin_lifecycle::stage_plugin_version)
                .delete(plugin_lifecycle::discard_staged_plugin_version),
        )
        .route(
            "/api/v1/plugins/{id}/lifecycle/rollback",
            post(plugin_lifecycle::roll_back_plugin_version),
        )
        .route(
            "/api/v1/plugins/{id}/lifecycle/policy",
            axum::routing::put(plugin_lifecycle::set_plugin_update_policy),
        )
        .route(
            "/api/v1/plugins/{id}/lifecycle/trial",
            post(plugin_lifecycle::trial_staged_plugin_version),
        )
        .route(
            "/api/v1/plugins/keys",
            get(plugin_handlers::list_plugin_keys),
        )
        .route(
            "/api/v1/plugins/keys/{key_id}",
            delete(plugin_handlers::revoke_plugin_key),
        )
        // A static segment beats `/{id}` and `/{id}/{version}` in matchit, so these sit
        // beside the key routes without a plugin id ever being able to shadow them.
        .route(
            "/api/v1/plugins/revocations",
            get(plugin_handlers::list_plugin_revocations)
                .post(plugin_handlers::revoke_plugin_digest),
        )
        .route(
            "/api/v1/plugins/revocations/{digest}",
            delete(plugin_handlers::unrevoke_plugin_digest),
        )
        .route(
            "/api/v1/plugins/i18n/{locale}",
            get(plugin_handlers::plugin_messages),
        )
        // Static segments again, so `repositories`, `preview` and `updates` are never read as a
        // plugin id (RD-140-01).
        .route(
            "/api/v1/plugins/preview",
            post(plugin_repository_handlers::preview_plugin_package),
        )
        // The bundle by service, installing from it (RD-160-05) and removing what the wizard
        // unticks (RD-180-14); static like the others.
        .route(
            "/api/v1/plugins/bundled",
            get(plugin_bundled::list_bundled_services),
        )
        .route(
            "/api/v1/plugins/bundled/install",
            post(plugin_bundled::install_bundled_services),
        )
        .route(
            "/api/v1/plugins/bundled/remove",
            post(plugin_bundled::remove_bundled_services),
        )
        .route(
            "/api/v1/plugins/updates",
            get(plugin_repository_handlers::list_plugin_updates),
        )
        .route(
            "/api/v1/plugins/updates/settings",
            get(plugin_update_policy::get_plugin_update_settings)
                .put(plugin_update_policy::set_plugin_update_settings),
        )
        .route(
            "/api/v1/plugins/repositories",
            get(plugin_repository_handlers::list_plugin_repositories)
                .post(plugin_repository_handlers::add_plugin_repository),
        )
        .route(
            "/api/v1/plugins/repositories/refresh",
            post(plugin_repository_handlers::refresh_plugin_repositories),
        )
        .route(
            "/api/v1/plugins/repositories/settings",
            put(plugin_repository_handlers::set_plugin_repository_settings),
        )
        .route(
            "/api/v1/plugins/repositories/{id}",
            patch(plugin_repository_handlers::update_plugin_repository)
                .delete(plugin_repository_handlers::remove_plugin_repository),
        )
        .route(
            "/api/v1/plugins/repositories/{id}/preview",
            post(plugin_repository_handlers::preview_repository_package),
        )
        .route(
            "/api/v1/plugins/repositories/{id}/install",
            post(plugin_repository_handlers::install_repository_package),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    plugin_handlers::list_plugins,
    plugin_handlers::remove_plugin_version,
    plugin_handlers::set_plugin_enabled,
    plugin_handlers::list_plugin_executions,
    plugin_handlers::install_plugin,
    plugin_handlers::list_plugin_keys,
    plugin_handlers::revoke_plugin_key,
    plugin_handlers::list_plugin_revocations,
    plugin_handlers::revoke_plugin_digest,
    plugin_handlers::unrevoke_plugin_digest,
    plugin_handlers::plugin_messages,
    plugin_bundled::list_bundled_services,
    plugin_bundled::install_bundled_services,
    plugin_bundled::remove_bundled_services,
    plugin_lifecycle::activate_plugin_version,
    plugin_lifecycle::stage_plugin_version,
    plugin_lifecycle::discard_staged_plugin_version,
    plugin_lifecycle::roll_back_plugin_version,
    plugin_lifecycle::set_plugin_update_policy,
    plugin_lifecycle::trial_staged_plugin_version,
    plugin_repository_handlers::list_plugin_repositories,
    plugin_repository_handlers::add_plugin_repository,
    plugin_repository_handlers::update_plugin_repository,
    plugin_repository_handlers::remove_plugin_repository,
    plugin_repository_handlers::refresh_plugin_repositories,
    plugin_repository_handlers::set_plugin_repository_settings,
    plugin_repository_handlers::list_plugin_updates,
    plugin_update_policy::get_plugin_update_settings,
    plugin_update_policy::set_plugin_update_settings,
    plugin_repository_handlers::preview_plugin_package,
    plugin_repository_handlers::preview_repository_package,
    plugin_repository_handlers::install_repository_package,
))]
pub(crate) struct Doc;
