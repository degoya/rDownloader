//! The mirror choice of the LinkGrabber: the standing preference, a pin per group and the
//! dissolving of a proposed group (RD-110-19, RD-110-34).

use axum::{
    Json,
    extract::{Path, State},
};
use rd_core::CandidateId;

use super::message;
use crate::{ApiError, AppState, dto::MessageResponse};

/// The standing mirror preference (RD-110-19).
///
/// Its own route rather than a field of the settings document: it is set from the LinkGrabber
/// toolbar while the settings page may be open elsewhere, and the settings document is written
/// back whole.
#[utoipa::path(get, path = "/api/v1/collector/mirror-preference", tag = "collector", responses((status = 200, body = rd_core::MirrorPreference)))]
pub async fn get_mirror_preference(
    State(state): State<AppState>,
) -> Result<Json<rd_core::MirrorPreference>, ApiError> {
    Ok(Json(state.database.mirror_preference().await?))
}

/// Stores the standing mirror preference and re-chooses every group under it.
///
/// The re-choice happens here rather than in the browser because the choice is what the queue
/// will fetch: a preference the interface has applied to its own rows while the stored rows
/// still name the previous mirror is how somebody queues the one they moved away from.
#[utoipa::path(put, path = "/api/v1/collector/mirror-preference", tag = "collector", request_body = rd_core::MirrorPreference, responses((status = 200, body = rd_core::MirrorPreference)))]
pub async fn put_mirror_preference(
    State(state): State<AppState>,
    Json(request): Json<rd_core::MirrorPreference>,
) -> Result<Json<rd_core::MirrorPreference>, ApiError> {
    let preference = rd_core::MirrorPreference {
        quality: trimmed_facet(request.quality),
        language: trimmed_facet(request.language),
        hoster: trimmed_facet(request.hoster),
        hidden_hosters: normalized_hosters(request.hidden_hosters),
    };
    if preference.hidden_hosters.len() > MAX_HIDDEN_HOSTERS {
        return Err(ApiError::bad_request(
            "collector.hidden_hosters_count",
            "At most 256 hosters can be hidden",
        )
        .with_param("max", MAX_HIDDEN_HOSTERS));
    }
    // A host name has at most 253 characters; anything longer is not one of the list's hosters.
    if preference
        .hidden_hosters
        .iter()
        .any(|hoster| hoster.chars().count() > MAX_HOSTER)
    {
        return Err(ApiError::bad_request(
            "collector.hidden_hoster_length",
            "A hidden hoster must be at most 253 characters",
        )
        .with_param("max", MAX_HOSTER));
    }
    for value in [
        &preference.quality,
        &preference.language,
        &preference.hoster,
    ]
    .into_iter()
    .flatten()
    {
        // A facet is a token off a release name or a host; nothing legitimate is longer, and
        // the value is compared against every candidate of every package on every regroup.
        if value.chars().count() > MAX_FACET {
            return Err(ApiError::bad_request(
                "collector.mirror_facet_length",
                "A mirror facet must be at most 64 characters",
            )
            .with_param("max", MAX_FACET));
        }
    }
    state
        .database
        .set_mirror_preference(preference.clone())
        .await?;
    Ok(Json(preference))
}

/// Makes one link its mirror group's chosen mirror, or releases that choice.
///
/// A pin outranks the preference and survives a regroup, which is what makes it the per-package
/// way out of a standing default.
#[utoipa::path(post, path = "/api/v1/collector/candidates/{id}/mirror", tag = "collector", params(("id" = rd_core::CandidateId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn pin_mirror(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<MessageResponse>, ApiError> {
    set_mirror_pin(&state, id, true).await
}

#[utoipa::path(delete, path = "/api/v1/collector/candidates/{id}/mirror", tag = "collector", params(("id" = rd_core::CandidateId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn release_mirror(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<MessageResponse>, ApiError> {
    set_mirror_pin(&state, id, false).await
}

/// Takes a proposed mirror group apart, so its links stand on their own again (RD-110-34).
///
/// Only a proposal: a group a page declared, or one two links corroborated with a matching
/// size, is refused here rather than asked about. A contradiction against those two is a
/// finding about the source, and the answer to it is to fix the rule, not to click the group
/// away in one package and meet it again on the next page.
#[utoipa::path(post, path = "/api/v1/collector/candidates/{id}/mirror/dissolve", tag = "collector", params(("id" = rd_core::CandidateId, Path)), responses((status = 200, body = MessageResponse), (status = 404), (status = 409)))]
pub async fn dissolve_mirror(
    State(state): State<AppState>,
    Path(id): Path<CandidateId>,
) -> Result<Json<MessageResponse>, ApiError> {
    match state.database.dissolve_mirror_group(id).await? {
        rd_db::MirrorDissolve::Dissolved => Ok(message(
            "collector.mirror_dissolved",
            "Mirror group dissolved",
        )),
        rd_db::MirrorDissolve::NotProposed => Err(ApiError::conflict(
            "collector.mirror_group_not_proposed",
            "This mirror group is not a proposal and cannot be dissolved",
        )),
        rd_db::MirrorDissolve::NotGrouped => Err(ApiError::not_found(
            "collector.mirror_not_grouped",
            "This link is not part of a mirror group",
        )),
    }
}

/// Longest a single facet value may be.
const MAX_FACET: usize = 64;
/// The hosters one LinkGrabber can hide at once (RD-130-21); far more than a list ever holds.
const MAX_HIDDEN_HOSTERS: usize = 256;
/// The longest host name DNS allows.
const MAX_HOSTER: usize = 253;

/// An empty facet is no facet: a select that was cleared sends `""`, and storing that would
/// make the preference match nothing and hide the whole list.
fn trimmed_facet(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Hidden hosters in the form the grouping compares them in (RD-130-21): the shared
/// [`rd_core::host_key`] (trimmed, lowercased, without a trailing dot or a leading `www.`;
/// audit 1.9.1, INTAKE-11) — the reduction `hosterOf` makes in the interface — each once and
/// sorted, so the stored list reads the same however it was sent.
fn normalized_hosters(values: Vec<String>) -> Vec<String> {
    let hosters: std::collections::BTreeSet<String> = values
        .into_iter()
        .filter_map(|value| {
            let value = rd_core::host_key(&value);
            (!value.is_empty()).then_some(value)
        })
        .collect();
    hosters.into_iter().collect()
}

async fn set_mirror_pin(
    state: &AppState,
    id: CandidateId,
    pinned: bool,
) -> Result<Json<MessageResponse>, ApiError> {
    if state.database.set_mirror_pin(id, pinned).await? {
        return Ok(message(
            if pinned {
                "collector.mirror_pinned"
            } else {
                "collector.mirror_released"
            },
            if pinned {
                "Mirror chosen"
            } else {
                "Mirror choice released"
            },
        ));
    }
    Err(ApiError::not_found(
        "collector.mirror_not_grouped",
        "This link is not part of a mirror group",
    ))
}
