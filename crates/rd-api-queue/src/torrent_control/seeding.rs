//! The seeding policy of one torrent download and of a category.

use super::*;

/// A seeding override as the API takes it.
///
/// Every field is optional twice over: absent means "inherit", and an explicit `null`
/// clears a value that was set before. That is the only way to express "stop overriding
/// this one field" in a single request.
#[derive(Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(default)]
pub struct SeedingPolicyRequest {
    pub enabled: Option<bool>,
    pub ratio: Option<f64>,
    /// Minutes to seed; `null` with `time_unlimited` unset leaves the field inheriting.
    pub time_minutes: Option<u32>,
    /// Seed without a time limit, overriding an inherited one.
    pub time_unlimited: Option<bool>,
}

impl SeedingPolicyRequest {
    /// Validates and converts the request into a stored override.
    pub(super) fn into_override(self) -> Result<rd_core::SeedingPolicyOverride, ApiError> {
        if let Some(ratio) = self.ratio
            && (!ratio.is_finite()
                || !(rd_core::MIN_SEED_RATIO..=rd_core::MAX_SEED_RATIO).contains(&ratio))
        {
            return Err(ApiError::bad_request(
                "torrent.seeding_policy_invalid",
                "Seed ratio must be between 0 and 100",
            )
            .with_param("min", rd_core::MIN_SEED_RATIO)
            .with_param("max", rd_core::MAX_SEED_RATIO));
        }
        if self
            .time_minutes
            .is_some_and(|minutes| minutes == 0 || minutes > rd_core::MAX_SEED_TIME_MINUTES)
        {
            return Err(ApiError::bad_request(
                "torrent.seeding_policy_invalid",
                "Seed time must be between 1 minute and one year",
            ));
        }
        let mut policy = rd_core::SeedingPolicyOverride {
            enabled: self.enabled,
            ratio_milli: None,
            time: match (self.time_unlimited, self.time_minutes) {
                (Some(true), _) => Some(rd_core::SeedTimeLimit::Unlimited),
                (_, Some(minutes)) => Some(rd_core::SeedTimeLimit::Minutes(minutes)),
                _ => None,
            },
        };
        policy.set_ratio(self.ratio);
        Ok(policy)
    }
}

/// The effective policy of one torrent, with the source of every field.
#[derive(Debug, Serialize, ToSchema)]
pub struct SeedingPolicyResponse {
    pub effective: rd_core::EffectiveSeedingPolicy,
    /// The override stored on this torrent, if any.
    pub torrent_override: Option<rd_core::SeedingPolicyOverride>,
}

/// Reads the seeding policy of one torrent.
#[utoipa::path(
    get,
    path = "/api/v1/downloads/{id}/torrent/seeding",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    responses(
        (status = 200, body = SeedingPolicyResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn get_download_seeding(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
) -> Result<Json<SeedingPolicyResponse>, ApiError> {
    let stored = require_download_state(&state, id).await?;
    Ok(Json(SeedingPolicyResponse {
        effective: state.torrent.effective_policy(id).await,
        torrent_override: (!stored.seeding.is_empty()).then_some(stored.seeding),
    }))
}

/// Replaces the seeding override of one torrent.
#[utoipa::path(
    put,
    path = "/api/v1/downloads/{id}/torrent/seeding",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    request_body = SeedingPolicyRequest,
    responses(
        (status = 200, body = SeedingPolicyResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn put_download_seeding(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
    Json(request): Json<SeedingPolicyRequest>,
) -> Result<Json<SeedingPolicyResponse>, ApiError> {
    let stored = apply_download_seeding(&state, id, request).await?;
    Ok(Json(SeedingPolicyResponse {
        effective: state.torrent.effective_policy(id).await,
        torrent_override: (!stored.seeding.is_empty()).then_some(stored.seeding),
    }))
}

/// Stores a seeding override on one torrent and lets the engine act on it now.
///
/// Split out so the qBittorrent adapter's `setShareLimits` writes the same override through
/// the same validation as the native endpoint, rather than acknowledging and forgetting it.
pub async fn apply_download_seeding(
    state: &AppState,
    id: DownloadId,
    request: SeedingPolicyRequest,
) -> Result<TorrentJobState, ApiError> {
    let mut stored = require_download_state(state, id).await?;
    stored.seeding = request.into_override()?;
    state
        .database
        .set_download_torrent_state(id, stored.clone())
        .await?;
    // A lowered limit must end a running seed now, not at the next tick.
    state.torrent.nudge_seeding();
    Ok(stored)
}

/// Clears the seeding override of one torrent, so it inherits again.
#[utoipa::path(
    delete,
    path = "/api/v1/downloads/{id}/torrent/seeding",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    responses(
        (status = 200, body = SeedingPolicyResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn delete_download_seeding(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
) -> Result<Json<SeedingPolicyResponse>, ApiError> {
    let mut stored = require_download_state(&state, id).await?;
    stored.seeding = rd_core::SeedingPolicyOverride::default();
    state
        .database
        .set_download_torrent_state(id, stored)
        .await?;
    state.torrent.nudge_seeding();
    Ok(Json(SeedingPolicyResponse {
        effective: state.torrent.effective_policy(id).await,
        torrent_override: None,
    }))
}

/// Replaces the seeding override of one category.
#[utoipa::path(
    put,
    path = "/api/v1/categories/{id}/seeding",
    tag = "configuration",
    params(("id" = rd_core::CategoryId, Path)),
    request_body = SeedingPolicyRequest,
    responses(
        (status = 200, body = crate::dto::MessageResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn put_category_seeding(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CategoryId>,
    Json(request): Json<SeedingPolicyRequest>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    let policy = request.into_override()?;
    state
        .database
        .set_category_seeding_policy(id, Some(policy))
        .await
        .map_err(|_| ApiError::not_found("category.not_found", "Category not found"))?;
    state.torrent.nudge_seeding();
    Ok(Json(crate::dto::MessageResponse::new(
        "torrent.seeding_policy_saved",
        "Seeding policy saved",
    )))
}

/// Clears the seeding override of one category.
#[utoipa::path(
    delete,
    path = "/api/v1/categories/{id}/seeding",
    tag = "configuration",
    params(("id" = rd_core::CategoryId, Path)),
    responses(
        (status = 200, body = crate::dto::MessageResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn delete_category_seeding(
    State(state): State<AppState>,
    Path(id): Path<rd_core::CategoryId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    state
        .database
        .set_category_seeding_policy(id, None)
        .await
        .map_err(|_| ApiError::not_found("category.not_found", "Category not found"))?;
    state.torrent.nudge_seeding();
    Ok(Json(crate::dto::MessageResponse::new(
        "torrent.seeding_policy_cleared",
        "Seeding policy cleared",
    )))
}
