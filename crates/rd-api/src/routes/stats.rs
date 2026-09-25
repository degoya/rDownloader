//! The metrics exposition and the transfer statistics (RD-110-01).

use axum::{
    Router,
    routing::{get, post},
};
use utoipa::OpenApi;

use crate::{AppState, data_reset_handlers, metrics, stats_handlers};

/// Session- or token-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/metrics", get(metrics::scrape_metrics))
        .route(
            "/api/v1/stats/transfers",
            get(stats_handlers::transfer_stats),
        )
        .route(
            "/api/v1/stats/transfers/clear",
            post(data_reset_handlers::clear_transfer_stats),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    metrics::scrape_metrics,
    stats_handlers::transfer_stats,
    data_reset_handlers::clear_transfer_stats,
))]
pub(crate) struct Doc;
