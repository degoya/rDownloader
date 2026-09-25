//! Plugin inventory, installation, diagnostics and trusted-key routes.

use axum::{
    Router,
    routing::{delete, get, post},
};
use utoipa::OpenApi;

use crate::{AppState, plugin_handlers};

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
))]
pub(crate) struct Doc;
