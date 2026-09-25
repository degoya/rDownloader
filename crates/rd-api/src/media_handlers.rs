//! The media format selector: fetching a candidate's format inventory, previewing what a
//! set of criteria resolves to, and storing the choice (RD-080-01).
//!
//! Resolving happens here rather than in the database layer because it needs the
//! extractor-aware code in `rd-media`; what reaches the writer is an already-decided
//! variant plus the criteria it came from.

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::{
    CandidateId, MediaCandidateState, MediaFormatCriteria, MediaKind, MediaResolution,
    MediaSelectionUpdate, MediaTarget, MediaVariant, embed_warnings, track_warnings,
};
use rd_media::{FfmpegTools, MediaCapabilities, resolve};

use crate::{
    ApiError, AppState,
    media_dto::{
        MediaFormatsResponse, MediaOutputPreviewRequest, MediaOutputPreviewResponse,
        MediaResolutionResponse, MediaSelectionRequest, MediaToolCapabilities,
    },
};

/// The format inventory and current selection of one media candidate.
#[utoipa::path(
    get,
    path = "/api/v1/collector/candidates/{id}/media",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    responses(
        (status = 200, body = crate::media_dto::MediaFormatsResponse),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody)
    )
)]
pub async fn get_candidate_media_formats(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<MediaFormatsResponse>, ApiError> {
    let stored = state
        .database
        .candidate_media_state(id)
        .await?
        .ok_or_else(formats_missing)?;
    if stored.is_future_contract() {
        return Err(ApiError::conflict(
            "media.formats_future_contract",
            "This media selection was written by a newer version of rDownloader",
        ));
    }
    let capabilities = capabilities(&state).await;
    let page_url = candidate_url(&state, id).await.ok();
    // A refusal is part of the answer, not a failure of the request: the inventory is still
    // worth showing, and the code says why nothing resolves. It used to be dropped with
    // `.ok()`, which left the selector to guess "no combination fits" (RD-120-50).
    let (resolved, unresolved_code) =
        match resolve(&stored.inventory, &stored.criteria, capabilities) {
            Ok(resolution) => (
                Some(response(
                    &resolution,
                    &stored.criteria,
                    &stored,
                    capabilities,
                    page_url.as_ref(),
                )),
                None,
            ),
            Err(error) => (None, Some(error.code().to_owned())),
        };
    Ok(Json(MediaFormatsResponse {
        inventory: stored.inventory,
        criteria: stored.criteria,
        resolved,
        unresolved_code,
        capabilities: MediaToolCapabilities {
            can_merge: capabilities.can_merge,
            can_transcode_audio: capabilities.can_transcode_audio,
        },
        audio_tracks: stored.audio_tracks,
        subtitles: stored.subtitles,
    }))
}

/// What a set of criteria would resolve to, without storing anything.
///
/// This is what drives the live "n of m formats match" counter and the explanation shown
/// when a filter combination keeps nothing.
#[utoipa::path(
    post,
    path = "/api/v1/collector/candidates/{id}/media/preview",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    request_body = MediaSelectionRequest,
    responses(
        (status = 200, body = crate::media_dto::MediaResolutionResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn preview_media_selection(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<MediaSelectionRequest>,
) -> Result<Json<MediaResolutionResponse>, ApiError> {
    let (stored, criteria, capabilities, page_url) = prepare(&state, id, request).await?;
    let resolution =
        resolve(&stored.inventory, &criteria, capabilities).map_err(selection_error)?;
    Ok(Json(response(
        &resolution,
        &criteria,
        &stored,
        capabilities,
        Some(&page_url),
    )))
}

/// What an output template expands to for one candidate.
///
/// Previewed against the candidate's real probe metadata and through the *same* evaluator
/// the download uses, so the path shown here is the path that gets written. An invalid
/// template is reported with the reason rather than silently producing something else.
#[utoipa::path(
    post,
    path = "/api/v1/collector/candidates/{id}/media/output-preview",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    request_body = MediaOutputPreviewRequest,
    responses(
        (status = 200, body = crate::media_dto::MediaOutputPreviewResponse),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn preview_media_output(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<MediaOutputPreviewRequest>,
) -> Result<Json<MediaOutputPreviewResponse>, ApiError> {
    let candidate = state
        .database
        .get_candidate(id)
        .await?
        .ok_or_else(|| ApiError::not_found("collector.candidate_missing", "Link not found"))?;
    let media = candidate.media.as_ref().ok_or_else(|| {
        ApiError::not_found("media.not_media_link", "This link is not a media page")
    })?;
    let template = request.template.trim();
    let fields = rd_files::TEMPLATE_FIELDS
        .iter()
        .map(|field| (*field).to_owned())
        .collect();
    let file_name = candidate
        .file_name
        .clone()
        .unwrap_or_else(|| media.title.clone());
    if template.is_empty() {
        return Ok(Json(MediaOutputPreviewResponse {
            relative_path: file_name,
            fields,
        }));
    }
    // A relative base: the preview describes a path inside the package directory, and the
    // real destination is not known until the link is queued.
    let base = std::path::Path::new("");
    let path = rd_files::expand(base, template, &template_values(media), 0)
        .map_err(|error| ApiError::bad_request("media.template_invalid", error.to_string()))?;
    let extension = media
        .selected_variant()
        .map_or("mp4", |variant| variant.ext.as_str());
    Ok(Json(MediaOutputPreviewResponse {
        relative_path: format!("{}.{extension}", path.to_string_lossy().replace('\\', "/")),
        fields,
    }))
}

/// The allowlisted fields of one probed page.
///
/// Kept beside the preview rather than in `rd-media`, because it maps *stored candidate*
/// metadata; the runner maps the selection instead, and the two carry different things.
fn template_values(media: &rd_core::MediaInfo) -> rd_files::TemplateValues {
    let mut values = rd_files::TemplateValues::new();
    values.insert("title".to_owned(), media.title.clone());
    if let Some(uploader) = media.uploader.as_deref().filter(|value| !value.is_empty()) {
        values.insert("uploader".to_owned(), uploader.to_owned());
    }
    if let Some(date) = media
        .upload_date
        .as_deref()
        .filter(|value| value.chars().count() >= 4)
    {
        values.insert("upload_date".to_owned(), date.to_owned());
        values.insert(
            "upload_year".to_owned(),
            date.chars().take(4).collect::<String>(),
        );
    }
    if let Some(extractor) = media.extractor.as_deref().filter(|value| !value.is_empty()) {
        values.insert("extractor".to_owned(), extractor.to_owned());
    }
    if let Some(id) = media.video_id.as_deref().filter(|value| !value.is_empty()) {
        values.insert("id".to_owned(), id.to_owned());
    }
    if let Some(variant) = media.selected_variant() {
        values.insert("ext".to_owned(), variant.ext.clone());
        if let Some(height) = variant.height {
            values.insert("resolution".to_owned(), format!("{height}p"));
        }
    }
    values
}

/// Stores a selection on a media candidate.
#[utoipa::path(
    put,
    path = "/api/v1/collector/candidates/{id}/media/selection",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    request_body = MediaSelectionRequest,
    responses(
        (status = 200, body = rd_core::LinkCandidate),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn put_candidate_media_selection(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<MediaSelectionRequest>,
) -> Result<Json<rd_core::LinkCandidate>, ApiError> {
    let (stored, criteria, capabilities, _) = prepare(&state, id, request).await?;
    let resolution =
        resolve(&stored.inventory, &criteria, capabilities).map_err(selection_error)?;
    let variant = variant(&resolution, &criteria);
    let candidate = state
        .database
        .set_candidate_media_selection(id, MediaSelectionUpdate { criteria, variant })
        .await?;
    Ok(Json(candidate))
}

/// Loads the candidate's inventory and validates the requested criteria.
async fn prepare(
    state: &AppState,
    id: CandidateId,
    request: MediaSelectionRequest,
) -> Result<
    (
        MediaCandidateState,
        MediaFormatCriteria,
        MediaCapabilities,
        url::Url,
    ),
    ApiError,
> {
    let stored = state
        .database
        .candidate_media_state(id)
        .await?
        .ok_or_else(formats_missing)?;
    let page_url = candidate_url(state, id).await?;
    let capabilities = capabilities(state).await;
    let criteria = match request.criteria {
        Some(criteria) => criteria,
        None => {
            let preset = request.preset.as_deref().unwrap_or("best");
            MediaFormatCriteria::preset(preset).ok_or_else(|| {
                ApiError::bad_request(
                    "media.criteria_invalid",
                    format!("'{preset}' is not a known media preset"),
                )
            })?
        }
    };
    if let Some(template) = criteria.output_template.as_deref() {
        rd_files::validate(template)
            .map_err(|error| ApiError::bad_request("media.template_invalid", error.to_string()))?;
    }
    let criteria = MediaFormatCriteria {
        // Merging is a property of the installation, not of the request: honouring a
        // client that asks for it on a machine without ffmpeg would produce two unusable
        // stream files.
        allow_merge: criteria.allow_merge && capabilities.can_merge,
        ..criteria
    }
    .sanitized()
    .map_err(|error| ApiError::bad_request("media.criteria_invalid", error.to_string()))?;
    Ok((stored, criteria, capabilities, page_url))
}

/// The page a candidate points at. Needed by the embed check, which refuses to write a
/// signed URL into a file.
async fn candidate_url(state: &AppState, id: CandidateId) -> Result<url::Url, ApiError> {
    Ok(state
        .database
        .get_candidate(id)
        .await?
        .ok_or_else(|| ApiError::not_found("collector.candidate_missing", "Link not found"))?
        .url)
}

/// What the installed tools allow.
async fn capabilities(state: &AppState) -> MediaCapabilities {
    let settings = state.media_settings.read().await.clone();
    let complete = FfmpegTools::resolve(&settings).is_complete();
    MediaCapabilities {
        can_merge: complete,
        can_transcode_audio: complete,
    }
}

/// The variant a resolution stores on the candidate.
fn variant(resolution: &MediaResolution, criteria: &MediaFormatCriteria) -> MediaVariant {
    let primary = resolution.video.as_ref();
    MediaVariant {
        id: criteria
            .preset
            .clone()
            .unwrap_or_else(|| "custom".to_owned()),
        label: label(resolution),
        kind: if criteria.target == MediaTarget::AudioOnly {
            MediaKind::Audio
        } else {
            MediaKind::Video
        },
        ext: resolution.container.clone(),
        height: primary.and_then(|format| format.height),
        abr: resolution
            .audio
            .as_ref()
            .and_then(|format| format.audio_bitrate_kbps)
            .or_else(|| primary.and_then(|format| format.audio_bitrate_kbps)),
        filesize_approx: resolution.estimated_bytes,
        format: resolution.format_expression.clone(),
        fps: primary.and_then(|format| format.fps),
        dynamic_range: primary
            .map(|format| format.dynamic_range)
            .unwrap_or_default(),
        video_codec: primary.and_then(|format| format.video_codec),
        audio_codec: resolution
            .audio
            .as_ref()
            .and_then(|format| format.audio_codec)
            .or_else(|| primary.and_then(|format| format.audio_codec)),
        requires_merge: resolution.audio.is_some(),
        warnings: resolution.warnings.clone(),
        criteria: Some(criteria.clone()),
    }
}

/// `AV1 1080p60 HDR10 · mp4` — built from what was actually chosen, not from what was asked.
fn label(resolution: &MediaResolution) -> String {
    let Some(format) = resolution.video.as_ref() else {
        return resolution.container.clone();
    };
    let mut parts = Vec::new();
    if let Some(codec) = format.video_codec_raw.as_deref() {
        parts.push(codec.split('.').next().unwrap_or(codec).to_owned());
    }
    if let Some(height) = format.height {
        parts.push(match format.fps {
            Some(fps) if fps > 30 => format!("{height}p{fps}"),
            _ => format!("{height}p"),
        });
    }
    if format.dynamic_range.is_hdr() {
        parts.push(format!("{:?}", format.dynamic_range).to_uppercase());
    }
    if parts.is_empty() {
        parts.push(format.format_id.clone());
    }
    format!("{} · {}", parts.join(" "), resolution.container)
}

fn response(
    resolution: &MediaResolution,
    criteria: &MediaFormatCriteria,
    stored: &MediaCandidateState,
    capabilities: MediaCapabilities,
    page_url: Option<&url::Url>,
) -> MediaResolutionResponse {
    MediaResolutionResponse {
        format_expression: resolution.format_expression.clone(),
        container: resolution.container.clone(),
        estimated_bytes: resolution.estimated_bytes,
        label: label(resolution),
        relaxations: resolution
            .relaxations
            .iter()
            .map(|criterion| criterion.as_str().to_owned())
            .collect(),
        warnings: resolution.warnings.clone(),
        matched_counts: resolution.matched_counts.clone(),
        matched_total: resolution.matched_total,
        candidate_total: resolution.candidate_total,
        variant: variant(resolution, criteria),
        track_warnings: track_warnings(
            &criteria.tracks,
            &resolution.container,
            &stored.audio_tracks,
            &stored.subtitles,
            capabilities.can_merge,
        ),
        embed_warnings: page_url.map_or_else(Vec::new, |url| {
            embed_warnings(
                &criteria.embed,
                &resolution.container,
                url,
                capabilities.can_transcode_audio,
            )
        }),
    }
}

fn formats_missing() -> ApiError {
    ApiError::not_found(
        "media.formats_missing",
        "This link has no stored media formats",
    )
}

/// Maps a resolver refusal onto its stable REST code, keeping the criteria that are to
/// blame so the UI can point at them.
fn selection_error(error: rd_core::MediaSelectionError) -> ApiError {
    let api = ApiError::unprocessable(error.code(), error.to_string());
    match &error {
        rd_core::MediaSelectionError::NoMatch { unsatisfiable, .. } => api.with_param(
            "criteria",
            unsatisfiable
                .iter()
                .map(|criterion| criterion.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ),
        rd_core::MediaSelectionError::NoFormats
        | rd_core::MediaSelectionError::MergeRequired
        | rd_core::MediaSelectionError::NoAudio => api,
    }
}

/// Sets the cookie/authentication profile a candidate is queued with (RD-080-04).
///
/// Validated here rather than in the store: a pinned profile that does not exist, is
/// disabled, or does not cover the link's host is a mistake worth reporting while the user
/// is still looking at the link, not one to discover when the download fails.
#[utoipa::path(
    put,
    path = "/api/v1/collector/candidates/{id}/auth-profile",
    tag = "collector",
    params(("id" = CandidateId, Path)),
    request_body = crate::media_dto::CandidateAuthProfileRequest,
    responses(
        (status = 200, body = rd_core::LinkCandidate),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn put_candidate_auth_profile(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
    Json(request): Json<crate::media_dto::CandidateAuthProfileRequest>,
) -> Result<Json<rd_core::LinkCandidate>, ApiError> {
    use crate::media_dto::CandidateAuthProfileMode;

    let candidate =
        state.database.get_candidate(id).await?.ok_or_else(|| {
            ApiError::not_found("collector.candidate_not_found", "Link not found")
        })?;

    let selection = match request.mode {
        CandidateAuthProfileMode::Auto => rd_core::AuthProfileSelection::Auto,
        CandidateAuthProfileMode::None => rd_core::AuthProfileSelection::None,
        CandidateAuthProfileMode::Pinned => {
            let profile_id = request.profile_id.ok_or_else(|| {
                ApiError::bad_request(
                    "collector.auth_profile_missing",
                    "Pinning a profile requires a profile id",
                )
            })?;
            let profile = state
                .database
                .auth_profile(profile_id)
                .await?
                .ok_or_else(|| {
                    ApiError::not_found(
                        "collector.auth_profile_not_found",
                        "Authentication profile not found",
                    )
                })?;
            if !profile.enabled {
                return Err(ApiError::unprocessable(
                    "collector.auth_profile_disabled",
                    "That authentication profile is disabled",
                ));
            }
            // Refused up front: a profile scoped to another site would be materialised into
            // an empty cookie file and fail at download time, which is a worse place to
            // learn about it.
            if !profile.scope.matches_url(&candidate.url) {
                return Err(ApiError::unprocessable(
                    "collector.auth_profile_scope_mismatch",
                    "That profile does not cover this link's address",
                )
                .with_param("host", profile.scope.host.clone()));
            }
            rd_core::AuthProfileSelection::Pinned(profile_id)
        }
    };
    let candidate = state
        .database
        .set_candidate_auth_profile(id, selection)
        .await
        .map_err(|error| ApiError::conflict("collector.candidate_busy", error.to_string()))?;
    Ok(Json(candidate))
}
