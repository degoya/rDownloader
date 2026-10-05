//! A package's own speed limit (RD-1100-01), set in the package editor.
//!
//! It is the narrowest bucket of the limiter chain: the global, hand-set and profile limits
//! still apply on top, so the strictest of them binds. A torrent is not paced by that chain —
//! the engine owns its sockets — and librqbit cannot limit one torrent on its own, so a package
//! holding a torrent refuses a limit with `torrent.capability_unsupported` rather than taking
//! one that would do nothing.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use rd_core::{ByteCount, DownloadKind, PackageId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

/// A package's own download limit.
#[derive(Debug, Serialize, ToSchema)]
pub struct PackageSpeedLimitResponse {
    pub package_id: PackageId,
    /// The package's own download limit; `null` when it has none and only the global, hand-set
    /// and profile limits apply.
    pub download_bytes_per_second: Option<ByteCount>,
    /// Whether a limit can reach this package: `false` while it holds a torrent and the engine
    /// cannot limit one torrent on its own (`per_torrent_limits` in the torrent capabilities).
    pub supported: bool,
}

/// Sets or removes a package's own download limit.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default)]
pub struct PackageSpeedLimitRequest {
    /// Bytes per second, greater than zero; `null` removes the package's own limit.
    pub download_bytes_per_second: Option<ByteCount>,
}

/// Reads one package's own download limit.
#[utoipa::path(
    get,
    path = "/api/v1/packages/{id}/speed-limit",
    tag = "downloads",
    params(("id" = rd_core::PackageId, Path)),
    responses(
        (status = 200, body = PackageSpeedLimitResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn get_package_speed_limit(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<PackageId>,
) -> Result<Json<PackageSpeedLimitResponse>, ApiError> {
    Ok(Json(view(&state, id).await?))
}

/// Sets or removes one package's own download limit; it applies to running transfers at once.
#[utoipa::path(
    put,
    path = "/api/v1/packages/{id}/speed-limit",
    tag = "downloads",
    params(("id" = rd_core::PackageId, Path)),
    request_body = PackageSpeedLimitRequest,
    responses(
        (status = 200, body = PackageSpeedLimitResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn set_package_speed_limit(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<PackageId>,
    Json(request): Json<PackageSpeedLimitRequest>,
) -> Result<Json<PackageSpeedLimitResponse>, ApiError> {
    let rate = request.download_bytes_per_second.map(ByteCount::get);
    if rate == Some(0) {
        return Err(ApiError::bad_request(
            "bandwidth.package_limit_invalid",
            "A package speed limit must be greater than zero; send null to remove it",
        ));
    }
    let current = view(&state, id).await?;
    // Removing a limit is always allowed; only a new one has to be able to work.
    if rate.is_some() && !current.supported {
        return Err(crate::torrent_control::unsupported("per_torrent_limits"));
    }
    state.database.set_package_speed_limit(id, rate).await?;
    // Read back by the registry right away, so running transfers take the new quota now.
    state.scheduler.reload_bandwidth().await?;
    Ok(Json(view(&state, id).await?))
}

async fn view(state: &AppState, id: PackageId) -> Result<PackageSpeedLimitResponse, ApiError> {
    if state.database.get_package(id).await?.is_none() {
        return Err(crate::error_codes::package_not_found());
    }
    let holds_torrent = state
        .database
        .downloads_for_package(id)
        .await?
        .iter()
        .any(|file| file.kind == DownloadKind::Torrent);
    Ok(PackageSpeedLimitResponse {
        package_id: id,
        download_bytes_per_second: state
            .database
            .package_speed_limit(id)
            .await?
            .and_then(|rate| ByteCount::new(rate).ok()),
        supported: !holds_torrent || state.torrent.capabilities().per_torrent_limits,
    })
}
