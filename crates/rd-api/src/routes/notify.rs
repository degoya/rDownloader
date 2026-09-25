//! Notification hub, quiet hours/power and bandwidth routes.

use axum::{
    Router,
    routing::{get, post, put},
};
use utoipa::OpenApi;

use crate::{AppState, bandwidth_handlers, notify_handlers, power_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/bandwidth/profiles",
            get(bandwidth_handlers::list_profiles).post(bandwidth_handlers::create_profile),
        )
        .route(
            "/api/v1/bandwidth/profiles/{id}",
            put(bandwidth_handlers::update_profile).delete(bandwidth_handlers::delete_profile),
        )
        .route(
            "/api/v1/bandwidth/schedule",
            get(bandwidth_handlers::get_schedule).put(bandwidth_handlers::put_schedule),
        )
        .route(
            "/api/v1/bandwidth/status",
            get(bandwidth_handlers::bandwidth_status),
        )
        .route(
            "/api/v1/notifications/destinations",
            get(notify_handlers::list_destinations),
        )
        .route(
            "/api/v1/notifications/targets",
            get(notify_handlers::list_targets).post(notify_handlers::create_target),
        )
        .route(
            "/api/v1/notifications/targets/{id}",
            put(notify_handlers::update_target).delete(notify_handlers::delete_target),
        )
        .route(
            "/api/v1/notifications/targets/{id}/test",
            post(notify_handlers::test_target),
        )
        .route(
            "/api/v1/notifications/rules",
            get(notify_handlers::list_rules).post(notify_handlers::create_rule),
        )
        .route(
            "/api/v1/notifications/rules/{id}",
            put(notify_handlers::update_rule).delete(notify_handlers::delete_rule),
        )
        .route(
            "/api/v1/notifications/deliveries",
            get(notify_handlers::list_deliveries),
        )
        .route(
            "/api/v1/notifications/deliveries/clear",
            post(crate::data_reset_handlers::clear_notification_deliveries),
        )
        .route("/api/v1/power/status", get(power_handlers::power_status))
        .route(
            "/api/v1/reconnect",
            get(crate::reconnect_handlers::reconnect_status)
                .post(crate::reconnect_handlers::trigger_reconnect),
        )
        .route(
            "/api/v1/power/cancel",
            post(power_handlers::cancel_power_action),
        )
        .route(
            "/api/v1/bandwidth/capabilities",
            get(bandwidth_handlers::bandwidth_capabilities),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    bandwidth_handlers::list_profiles,
    bandwidth_handlers::create_profile,
    bandwidth_handlers::update_profile,
    bandwidth_handlers::delete_profile,
    bandwidth_handlers::get_schedule,
    bandwidth_handlers::put_schedule,
    bandwidth_handlers::bandwidth_status,
    bandwidth_handlers::bandwidth_capabilities,
    notify_handlers::list_destinations,
    notify_handlers::list_targets,
    notify_handlers::create_target,
    notify_handlers::update_target,
    notify_handlers::delete_target,
    notify_handlers::test_target,
    notify_handlers::list_rules,
    notify_handlers::create_rule,
    notify_handlers::update_rule,
    notify_handlers::delete_rule,
    notify_handlers::list_deliveries,
    crate::data_reset_handlers::clear_notification_deliveries,
    power_handlers::power_status,
    crate::reconnect_handlers::reconnect_status,
    crate::reconnect_handlers::trigger_reconnect,
    power_handlers::cancel_power_action,
))]
pub(crate) struct Doc;
