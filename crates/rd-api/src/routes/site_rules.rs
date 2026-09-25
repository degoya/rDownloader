//! The site rules a person can see, switch, write, try and carry (RD-110-08).

use axum::{
    Router,
    routing::{get, post, put},
};
use utoipa::OpenApi;

use crate::{AppState, site_rules_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/site-rules",
            get(site_rules_handlers::list_site_rules).post(site_rules_handlers::create_site_rule),
        )
        .route(
            "/api/v1/site-rules/export",
            get(site_rules_handlers::export_site_rules),
        )
        .route(
            "/api/v1/site-rules/import",
            post(site_rules_handlers::import_site_rules),
        )
        .route(
            "/api/v1/site-rules/test",
            post(site_rules_handlers::test_site_rule),
        )
        .route(
            "/api/v1/site-rules/{id}",
            put(site_rules_handlers::update_site_rule)
                .delete(site_rules_handlers::delete_site_rule),
        )
        .route(
            "/api/v1/site-rules/{id}/enabled",
            put(site_rules_handlers::set_site_rule_enabled),
        )
        .route(
            "/api/v1/site-rule-groups/{group}/enabled",
            put(site_rules_handlers::set_site_rule_group_enabled),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    site_rules_handlers::list_site_rules,
    site_rules_handlers::create_site_rule,
    site_rules_handlers::export_site_rules,
    site_rules_handlers::import_site_rules,
    site_rules_handlers::test_site_rule,
    site_rules_handlers::update_site_rule,
    site_rules_handlers::delete_site_rule,
    site_rules_handlers::set_site_rule_enabled,
    site_rules_handlers::set_site_rule_group_enabled,
))]
pub(crate) struct Doc;
