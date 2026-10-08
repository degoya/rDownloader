//! The site rules a person can see, switch, write, try and carry (RD-110-08).

use axum::{
    Router,
    routing::{get, post, put},
};
use utoipa::OpenApi;

use crate::{AppState, site_rule_picks, site_rules_handlers};

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
        // A series page's entries, chosen before they are resolved (RD-1170-03).
        .route(
            "/api/v1/collector/picks",
            get(site_rule_picks::list_collector_picks).post(site_rule_picks::create_collector_pick),
        )
        .route(
            "/api/v1/collector/picks/{id}",
            get(site_rule_picks::get_collector_pick).delete(site_rule_picks::delete_collector_pick),
        )
        .route(
            "/api/v1/collector/picks/{id}/resolve",
            post(site_rule_picks::resolve_collector_pick),
        )
        .route(
            "/api/v1/collector/picks/{id}/cancel",
            post(site_rule_picks::cancel_collector_pick),
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
    site_rule_picks::list_collector_picks,
    site_rule_picks::create_collector_pick,
    site_rule_picks::get_collector_pick,
    site_rule_picks::delete_collector_pick,
    site_rule_picks::resolve_collector_pick,
    site_rule_picks::cancel_collector_pick,
))]
pub(crate) struct Doc;
