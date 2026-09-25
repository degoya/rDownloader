//! Livestream channels, recording schedules and subscription routes.

use axum::{
    Router,
    routing::{get, post, put},
};
use utoipa::OpenApi;

use crate::{AppState, stream_handlers, stream_schedule_handlers, subscription_handlers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/subscriptions",
            get(subscription_handlers::list_subscriptions)
                .post(subscription_handlers::create_subscription),
        )
        .route(
            "/api/v1/subscriptions/{id}",
            put(subscription_handlers::update_subscription)
                .delete(subscription_handlers::delete_subscription),
        )
        .route(
            "/api/v1/subscriptions/{id}/enable",
            post(subscription_handlers::enable_subscription),
        )
        .route(
            "/api/v1/subscriptions/{id}/disable",
            post(subscription_handlers::disable_subscription),
        )
        .route(
            "/api/v1/subscriptions/{id}/caps",
            post(subscription_handlers::subscription_caps),
        )
        // Without an id: asks before there is a subscription to ask for.
        .route(
            "/api/v1/subscriptions/caps",
            post(subscription_handlers::probe_caps),
        )
        .route(
            "/api/v1/subscriptions/review-summary",
            get(subscription_handlers::subscription_review_summary),
        )
        .route(
            "/api/v1/subscriptions/export",
            get(crate::area_backup::export_subscriptions),
        )
        .route(
            "/api/v1/subscriptions/import",
            post(crate::area_backup::import_subscriptions),
        )
        .route(
            "/api/v1/streams/export",
            get(crate::area_backup::export_streams),
        )
        .route(
            "/api/v1/streams/import",
            post(crate::area_backup::import_streams),
        )
        .route(
            "/api/v1/subscriptions/{id}/poll",
            post(subscription_handlers::poll_subscription),
        )
        .route(
            "/api/v1/subscriptions/{id}/items",
            get(subscription_handlers::list_subscription_items),
        )
        .route(
            "/api/v1/subscriptions/{id}/items/page",
            get(subscription_handlers::list_subscription_item_page),
        )
        .route(
            "/api/v1/subscriptions/{id}/items/pending",
            put(subscription_handlers::set_pending_subscription_items_state),
        )
        .route(
            "/api/v1/subscriptions/{id}/history",
            axum::routing::delete(subscription_handlers::clear_subscription_history),
        )
        .route(
            "/api/v1/subscriptions/{id}/runs",
            get(subscription_handlers::list_subscription_runs),
        )
        .route(
            "/api/v1/subscriptions/items/{id}",
            put(subscription_handlers::set_subscription_item_state),
        )
        .route(
            "/api/v1/streams/schedules",
            get(stream_schedule_handlers::list_stream_schedules)
                .post(stream_schedule_handlers::create_stream_schedule),
        )
        .route(
            "/api/v1/streams/schedules/{id}",
            put(stream_schedule_handlers::update_stream_schedule)
                .delete(stream_schedule_handlers::delete_stream_schedule),
        )
        .route(
            "/api/v1/streams/runs",
            get(stream_schedule_handlers::list_stream_runs),
        )
        .route(
            "/api/v1/streams/channels",
            get(stream_handlers::list_stream_channels).post(stream_handlers::create_stream_channel),
        )
        .route(
            "/api/v1/streams/channels/{id}",
            axum::routing::put(stream_handlers::update_stream_channel)
                .delete(stream_handlers::delete_stream_channel),
        )
        .route("/api/v1/streams/record", post(stream_handlers::record_now))
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    subscription_handlers::list_subscriptions,
    subscription_handlers::create_subscription,
    subscription_handlers::update_subscription,
    subscription_handlers::enable_subscription,
    subscription_handlers::disable_subscription,
    subscription_handlers::delete_subscription,
    subscription_handlers::poll_subscription,
    subscription_handlers::subscription_caps,
    subscription_handlers::probe_caps,
    subscription_handlers::subscription_review_summary,
    crate::area_backup::export_subscriptions,
    crate::area_backup::import_subscriptions,
    crate::area_backup::export_streams,
    crate::area_backup::import_streams,
    subscription_handlers::list_subscription_items,
    subscription_handlers::list_subscription_item_page,
    subscription_handlers::list_subscription_runs,
    subscription_handlers::set_subscription_item_state,
    subscription_handlers::set_pending_subscription_items_state,
    subscription_handlers::clear_subscription_history,
    stream_schedule_handlers::list_stream_schedules,
    stream_schedule_handlers::create_stream_schedule,
    stream_schedule_handlers::update_stream_schedule,
    stream_schedule_handlers::delete_stream_schedule,
    stream_schedule_handlers::list_stream_runs,
    stream_handlers::list_stream_channels,
    stream_handlers::create_stream_channel,
    stream_handlers::update_stream_channel,
    stream_handlers::delete_stream_channel,
    stream_handlers::record_now,
))]
pub(crate) struct Doc;
