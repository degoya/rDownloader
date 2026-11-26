//! `GET <collection>/{id}` of the queue (RD-1480-10, API-6): one row as its list shows it.
//!
//! Every handler keeps one row of its list, or reads the row alone where the list grows, so the
//! two cannot drift apart in shape or in what they reveal (`rd_api_core::single_read`). The route
//! costs what the list costs.

use axum::extract::State;
use rd_api_core::extract::{Json, Path};
use rd_api_core::single_read::one_of;

use crate::{ApiError, AppState, error_codes::parse_id};

/// One download. What `POST /api/v1/downloads` created is followed here, without reading the
/// whole queue.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/downloads/{id}", tag = "downloads", params(("id" = rd_core::DownloadId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::DownloadFile), (status = 404)))]
pub async fn get_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::DownloadFile>, ApiError> {
    state
        .database
        .get_download(parse_id(&id)?)
        .await?
        .map(Json)
        .ok_or_else(crate::error_codes::download_not_found)
}

/// One package, with its archive password as the list shows it.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/packages/{id}", tag = "downloads", params(("id" = rd_core::PackageId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::DownloadPackage), (status = 404)))]
pub async fn get_package(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::DownloadPackage>, ApiError> {
    state
        .database
        .get_package_with_password(parse_id(&id)?)
        .await?
        .map(Json)
        .ok_or_else(crate::error_codes::package_not_found)
}

/// One bandwidth profile.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/bandwidth/profiles/{id}", tag = "bandwidth", params(("id" = rd_core::BandwidthProfileId, Path, description = "The id the list shows")), responses((status = 200, body = rd_limits::BandwidthProfile), (status = 404)))]
pub async fn get_bandwidth_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_limits::BandwidthProfile>, ApiError> {
    let id = parse_id::<rd_core::BandwidthProfileId>(&id)?;
    let Json(rows) = crate::bandwidth_handlers::list_profiles(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("bandwidth.profile_not_found", "Bandwidth profile not found"),
    )
}

/// One Usenet server, without its password.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/usenet/servers/{id}", tag = "usenet", params(("id" = rd_core::UsenetServerId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::UsenetServer), (status = 404)))]
pub async fn get_usenet_server(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::UsenetServer>, ApiError> {
    let id = parse_id::<rd_core::UsenetServerId>(&id)?;
    let Json(rows) = crate::usenet_handlers::list_usenet_servers(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        crate::error_codes::usenet_server_not_found,
    )
}
