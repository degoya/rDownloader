//! Reviewing and steering one torrent: the file tree before queueing and the same tree on
//! a running queue row.
//!
//! Selection lives in two places because it is edited in two places. A link candidate
//! carries it while the torrent is still being reviewed in the LinkGrabber; the queue row
//! carries it afterwards, where a change is also pushed to the running torrent.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::{
    CandidateId, DownloadId, ResolvedTorrentPlan, TorrentCandidateState, TorrentEngineCapabilities,
    TorrentFilePlan, TorrentFilePriority, TorrentJobState, TorrentMetadataInfo,
    TorrentMetadataState, TorrentSequentialMode,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{AppState, error::ApiError};

/// The torrent view of a link candidate or a queue row.
#[derive(Debug, Serialize, ToSchema)]
pub struct TorrentDetailResponse {
    pub metadata_state: TorrentMetadataState,
    /// Redaction-safe reason why metadata resolution failed.
    pub metadata_error: Option<String>,
    pub info_hash: Option<String>,
    pub name: Option<String>,
    /// Private torrents may not use DHT, PEX or LSD.
    pub private: bool,
    pub piece_count: u32,
    /// The resolved file tree; `None` while the metadata is still being fetched.
    pub plan: Option<ResolvedTorrentPlan>,
    /// BEP 19 web seeds advertised by the torrent, redacted.
    ///
    /// Diagnostics only: the engine has no web-seed support, which `capabilities.web_seeds`
    /// reports, so these URLs are shown but never fetched.
    pub web_seeds: Vec<String>,
    /// What the engine can do, so the UI can disable what it cannot.
    pub capabilities: TorrentEngineCapabilities,
}

impl TorrentDetailResponse {
    /// Builds the response from stored metadata and plan.
    fn new(
        metadata: Option<&TorrentMetadataInfo>,
        plan: &TorrentFilePlan,
        metadata_state: TorrentMetadataState,
        metadata_error: Option<String>,
        capabilities: TorrentEngineCapabilities,
    ) -> Self {
        Self {
            metadata_state,
            metadata_error,
            info_hash: metadata.map(|metadata| metadata.info_hash.clone()),
            name: metadata.map(|metadata| metadata.name.clone()),
            private: metadata.is_some_and(|metadata| metadata.private),
            piece_count: metadata.map_or(0, |metadata| metadata.piece_count),
            plan: metadata.map(|metadata| rd_core::resolve_plan(metadata, plan)),
            web_seeds: metadata
                .map(|metadata| {
                    metadata
                        .web_seeds
                        .iter()
                        .map(|url| rd_core::redact_tracker_url(url))
                        .collect()
                })
                .unwrap_or_default(),
            capabilities,
        }
    }
}

/// Replaces the selection of a torrent.
///
/// A full replacement rather than a patch: the tree is edited as a whole in the UI, and a
/// partial update would make the outcome depend on the order of requests.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default)]
pub struct TorrentPlanRequest {
    /// File indices the user explicitly included.
    pub included: Vec<u32>,
    /// File indices the user explicitly excluded.
    pub excluded: Vec<u32>,
    /// Per-file priority tiers. A folder priority is inherited onto its files by the
    /// client, so only per-file entries are ever stored.
    pub priorities: Vec<TorrentFilePriorityEntry>,
    /// Glob patterns excluding files the user has not decided explicitly.
    pub exclusion_patterns: Vec<String>,
    /// Streaming-oriented piece ordering; rejected unless the engine supports it.
    pub sequential: TorrentSequentialMode,
}

/// One file's priority.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct TorrentFilePriorityEntry {
    pub index: u32,
    pub priority: TorrentFilePriority,
}

impl TorrentPlanRequest {
    /// A request that only changes the inclusion of the named files.
    ///
    /// Used by the qBittorrent adapter, whose `filePrio` call expresses selection as a
    /// priority of zero. Everything else in the stored plan is carried forward by `apply`.
    #[must_use]
    pub(crate) fn selection(included: Vec<u32>, excluded: Vec<u32>) -> Self {
        Self {
            included,
            excluded,
            priorities: Vec::new(),
            exclusion_patterns: Vec::new(),
            sequential: TorrentSequentialMode::Off,
        }
    }

    /// Validates the request against the metadata and folds it into the stored plan.
    ///
    /// The existing plan is carried forward rather than replaced, so a selection change
    /// never silently drops priorities or exclusion patterns set earlier.
    fn apply(
        self,
        metadata: &TorrentMetadataInfo,
        mut plan: TorrentFilePlan,
        capabilities: TorrentEngineCapabilities,
    ) -> Result<TorrentFilePlan, ApiError> {
        if self.sequential != TorrentSequentialMode::Off {
            let capability = match self.sequential {
                TorrentSequentialMode::FirstLast => "first_last_piece",
                _ => "sequential_download",
            };
            if !capabilities.supports(capability) {
                return Err(unsupported(capability));
            }
        }
        if self.exclusion_patterns.len() > rd_core::MAX_EXCLUSION_PATTERNS {
            return Err(ApiError::bad_request(
                "torrent.pattern_invalid",
                format!(
                    "At most {} exclusion patterns are allowed",
                    rd_core::MAX_EXCLUSION_PATTERNS
                ),
            )
            .with_param("max", rd_core::MAX_EXCLUSION_PATTERNS.to_string()));
        }
        if let Some(pattern) = self.exclusion_patterns.iter().find(|pattern| {
            pattern.trim().is_empty() || pattern.len() > rd_core::MAX_EXCLUSION_PATTERN_LENGTH
        }) {
            return Err(ApiError::bad_request(
                "torrent.pattern_invalid",
                "An exclusion pattern is empty or too long",
            )
            .with_param("pattern", pattern.chars().take(64).collect::<String>()));
        }
        let unknown = self
            .included
            .iter()
            .chain(self.excluded.iter())
            .chain(self.priorities.iter().map(|entry| &entry.index))
            .find(|index| !metadata.contains_all([**index]));
        if let Some(index) = unknown {
            return Err(ApiError::bad_request(
                "torrent.file_index_unknown",
                format!("The torrent has no file with index {index}"),
            )
            .with_param("index", index.to_string()));
        }
        if let Some(index) = self
            .included
            .iter()
            .find(|index| self.excluded.contains(index))
        {
            return Err(ApiError::bad_request(
                "torrent.plan_invalid",
                format!("File {index} cannot be included and excluded at the same time"),
            )
            .with_param("index", index.to_string()));
        }
        plan.explicit = self
            .included
            .into_iter()
            .map(|index| (index, true))
            .chain(self.excluded.into_iter().map(|index| (index, false)))
            .collect();
        plan.priorities = self
            .priorities
            .into_iter()
            .map(|entry| (entry.index, entry.priority))
            .collect();
        plan.exclusion_patterns = self.exclusion_patterns;
        plan.sequential = self.sequential;
        let resolved = rd_core::resolve_plan(metadata, &plan);
        if resolved.included_indices().is_empty() {
            return Err(ApiError::bad_request(
                "torrent.plan_invalid",
                "At least one file must remain selected",
            ));
        }
        Ok(plan)
    }
}

/// Torrent details of one link candidate.
#[utoipa::path(
    get,
    path = "/api/v1/collector/candidates/{id}/torrent",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    responses(
        (status = 200, body = TorrentDetailResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn get_candidate_torrent(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<TorrentDetailResponse>, ApiError> {
    let stored = candidate_state(&state, id).await?;
    Ok(Json(TorrentDetailResponse::new(
        stored.metadata.as_ref(),
        &stored.plan,
        stored.metadata_state,
        stored.metadata_error,
        state.torrent.capabilities(),
    )))
}

/// Replaces the selection of a link candidate's torrent.
#[utoipa::path(
    put,
    path = "/api/v1/collector/candidates/{id}/torrent/plan",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    request_body = TorrentPlanRequest,
    responses(
        (status = 200, body = TorrentDetailResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn put_candidate_torrent_plan(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<TorrentPlanRequest>,
) -> Result<Json<TorrentDetailResponse>, ApiError> {
    let mut stored = candidate_state(&state, id).await?;
    let metadata = stored.metadata.clone().ok_or_else(metadata_pending)?;
    stored.plan = request.apply(&metadata, stored.plan, state.torrent.capabilities())?;
    state
        .database
        .set_candidate_torrent_state(id, stored.clone())
        .await?;
    Ok(Json(TorrentDetailResponse::new(
        Some(&metadata),
        &stored.plan,
        stored.metadata_state,
        stored.metadata_error,
        state.torrent.capabilities(),
    )))
}

/// Resolves the metadata behind a magnet link so it can be reviewed like a `.torrent`.
#[utoipa::path(
    post,
    path = "/api/v1/collector/candidates/{id}/torrent/resolve",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    responses(
        (status = 200, body = TorrentDetailResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn resolve_candidate_torrent(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<TorrentDetailResponse>, ApiError> {
    let mut stored = candidate_state(&state, id).await?;
    if stored.metadata.is_some() {
        return Ok(Json(TorrentDetailResponse::new(
            stored.metadata.as_ref(),
            &stored.plan,
            stored.metadata_state,
            stored.metadata_error,
            state.torrent.capabilities(),
        )));
    }
    let candidate =
        state.database.get_candidate(id).await?.ok_or_else(|| {
            ApiError::not_found("collector.candidate_not_found", "Link not found")
        })?;
    match state.torrent.resolve_metadata(&candidate.url).await {
        Ok(metadata) => {
            stored.metadata_state = TorrentMetadataState::Ready;
            stored.metadata_error = None;
            stored.metadata = Some(metadata);
        }
        Err(error) => {
            stored.metadata_state = TorrentMetadataState::Failed;
            // The magnet may carry a tracker URL with a passkey, so the message is built
            // from the error alone and never echoes the source.
            stored.metadata_error = Some(format!("{error:#}"));
        }
    }
    state
        .database
        .set_candidate_torrent_state(id, stored.clone())
        .await?;
    Ok(Json(TorrentDetailResponse::new(
        stored.metadata.as_ref(),
        &stored.plan,
        stored.metadata_state,
        stored.metadata_error,
        state.torrent.capabilities(),
    )))
}

/// Torrent details of one queue row.
#[utoipa::path(
    get,
    path = "/api/v1/downloads/{id}/torrent",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    responses(
        (status = 200, body = TorrentDetailResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn get_download_torrent(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
) -> Result<Json<TorrentDetailResponse>, ApiError> {
    let stored = require_download_state(&state, id).await?;
    Ok(Json(TorrentDetailResponse::new(
        stored.metadata.as_ref(),
        &stored.plan,
        metadata_state(&stored),
        None,
        state.torrent.capabilities(),
    )))
}

/// Replaces the selection of a queued torrent and pushes it to the running torrent.
#[utoipa::path(
    put,
    path = "/api/v1/downloads/{id}/torrent/plan",
    tag = "downloads",
    params(("id" = DownloadId, Path)),
    request_body = TorrentPlanRequest,
    responses(
        (status = 200, body = TorrentDetailResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn put_download_torrent_plan(
    State(state): State<AppState>,
    Path(id): Path<DownloadId>,
    Json(request): Json<TorrentPlanRequest>,
) -> Result<Json<TorrentDetailResponse>, ApiError> {
    let (metadata, stored) = apply_download_plan(&state, id, request).await?;
    Ok(Json(TorrentDetailResponse::new(
        Some(&metadata),
        &stored.plan,
        TorrentMetadataState::Ready,
        None,
        state.torrent.capabilities(),
    )))
}

/// Folds a plan request into a queued torrent and hands it to the engine.
///
/// Split out so the compatibility adapters change a selection through exactly the same
/// validation and engine call as the native endpoint.
pub(crate) async fn apply_download_plan(
    state: &AppState,
    id: DownloadId,
    request: TorrentPlanRequest,
) -> Result<(TorrentMetadataInfo, TorrentJobState), ApiError> {
    let mut stored = require_download_state(state, id).await?;
    let metadata = stored.metadata.clone().ok_or_else(metadata_pending)?;
    stored.plan = request.apply(&metadata, stored.plan, state.torrent.capabilities())?;
    state
        .torrent
        .apply_plan(id, stored.clone())
        .await
        .map_err(|error| ApiError::bad_request("torrent.plan_not_applied", format!("{error:#}")))?;
    Ok((metadata, stored))
}

/// Reads the stored candidate state, or the defaults when the row has none yet.
async fn candidate_state(
    state: &AppState,
    id: CandidateId,
) -> Result<TorrentCandidateState, ApiError> {
    match state.database.candidate_torrent_state(id).await? {
        Some(stored) => Ok(stored),
        None => {
            // Distinguish "not a torrent yet" from "no such link".
            state.database.get_candidate(id).await?.ok_or_else(|| {
                ApiError::not_found("collector.candidate_not_found", "Link not found")
            })?;
            Ok(TorrentCandidateState::default())
        }
    }
}

/// Reads the stored queue-row state, or the defaults when the row has none yet.
pub(crate) async fn require_download_state(
    state: &AppState,
    id: DownloadId,
) -> Result<TorrentJobState, ApiError> {
    match state.database.download_torrent_state(id).await? {
        Some(stored) => Ok(stored),
        None => {
            state
                .database
                .get_download(id)
                .await?
                .ok_or_else(crate::error_codes::download_not_found)?;
            Ok(TorrentJobState::default())
        }
    }
}

/// A queue row only has metadata once the engine resolved it.
fn metadata_state(state: &TorrentJobState) -> TorrentMetadataState {
    if state.metadata.is_some() {
        TorrentMetadataState::Ready
    } else {
        TorrentMetadataState::Pending
    }
}

/// The error returned for an option the engine cannot honour.
///
/// Reported rather than silently ignored, so a switch never appears to work when it does
/// nothing; `params.capability` names the missing feature.
pub(crate) fn unsupported(capability: &str) -> ApiError {
    ApiError::bad_request(
        "torrent.capability_unsupported",
        format!("The torrent engine does not support {capability}"),
    )
    .with_param("capability", capability.to_owned())
}

/// The error returned while a magnet's metadata is still unknown.
fn metadata_pending() -> ApiError {
    ApiError::bad_request(
        "torrent.metadata_pending",
        "The torrent metadata has not been resolved yet",
    )
}

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
    fn into_override(self) -> Result<rd_core::SeedingPolicyOverride, ApiError> {
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
pub(crate) async fn apply_download_seeding(
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
