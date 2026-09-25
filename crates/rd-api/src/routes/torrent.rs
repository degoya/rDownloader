//! Torrent file selection, trackers, peers and seeding routes.

use axum::{
    Router,
    routing::{get, post, put},
};
use utoipa::OpenApi;

use crate::{AppState, torrent_control, torrent_handlers, torrent_trackers};

/// Session-authenticated routes of this area.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/torrents/import",
            post(torrent_handlers::import_torrent),
        )
        .route(
            "/api/v1/torrents/capabilities",
            get(torrent_handlers::torrent_capabilities),
        )
        .route(
            "/api/v1/torrents/network/interfaces",
            get(torrent_handlers::torrent_interfaces),
        )
        .route(
            "/api/v1/torrents/network/status",
            get(torrent_handlers::torrent_network_status),
        )
        .route(
            "/api/v1/collector/candidates/{id}/torrent",
            get(torrent_control::get_candidate_torrent),
        )
        .route(
            "/api/v1/collector/candidates/{id}/torrent/plan",
            put(torrent_control::put_candidate_torrent_plan),
        )
        .route(
            "/api/v1/collector/candidates/{id}/torrent/resolve",
            post(torrent_control::resolve_candidate_torrent),
        )
        .route(
            "/api/v1/downloads/{id}/torrent",
            get(torrent_control::get_download_torrent),
        )
        .route(
            "/api/v1/downloads/{id}/torrent/plan",
            put(torrent_control::put_download_torrent_plan),
        )
        .route(
            "/api/v1/downloads/{id}/torrent/seeding",
            get(torrent_control::get_download_seeding)
                .put(torrent_control::put_download_seeding)
                .delete(torrent_control::delete_download_seeding),
        )
        .route(
            "/api/v1/categories/{id}/seeding",
            put(torrent_control::put_category_seeding)
                .delete(torrent_control::delete_category_seeding),
        )
        .route(
            "/api/v1/downloads/{id}/torrent/trackers",
            get(torrent_trackers::list_trackers).put(torrent_trackers::put_trackers),
        )
        .route(
            "/api/v1/downloads/{id}/torrent/trackers/reannounce",
            post(torrent_trackers::reannounce),
        )
        .route(
            "/api/v1/downloads/{id}/torrent/trackers/scrape",
            post(torrent_trackers::scrape_trackers),
        )
        .route(
            "/api/v1/downloads/{id}/torrent/stats",
            get(torrent_trackers::torrent_stats),
        )
        .route(
            "/api/v1/downloads/{id}/torrent/peers",
            get(torrent_trackers::torrent_peers),
        )
        .route(
            "/api/v1/downloads/{id}/torrent/pieces",
            get(torrent_trackers::torrent_pieces),
        )
        .route(
            "/api/v1/downloads/{id}/seeding/stop",
            post(torrent_handlers::stop_seeding),
        )
}

/// OpenAPI operations of this area.
#[derive(OpenApi)]
#[openapi(paths(
    torrent_control::get_candidate_torrent,
    torrent_control::get_download_torrent,
    torrent_control::put_candidate_torrent_plan,
    torrent_control::put_download_torrent_plan,
    torrent_control::resolve_candidate_torrent,
    torrent_control::get_download_seeding,
    torrent_control::put_download_seeding,
    torrent_control::delete_download_seeding,
    torrent_control::put_category_seeding,
    torrent_control::delete_category_seeding,
    torrent_handlers::import_torrent,
    torrent_handlers::stop_seeding,
    torrent_handlers::torrent_capabilities,
    torrent_handlers::torrent_interfaces,
    torrent_handlers::torrent_network_status,
    torrent_trackers::list_trackers,
    torrent_trackers::put_trackers,
    torrent_trackers::reannounce,
    torrent_trackers::scrape_trackers,
    torrent_trackers::torrent_peers,
    torrent_trackers::torrent_pieces,
    torrent_trackers::torrent_stats,
))]
pub(crate) struct Doc;
