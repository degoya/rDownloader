//! LinkFilter rules (RD-1240-09): list, create, change, reorder, delete, apply to the
//! LinkGrabber, and show what they hid.
//!
//! The rules decide at intake on their own; applying them is for the list as it already is —
//! after a rule changed, or after the online check learned the names and sizes a rule asks for.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use rd_core::LinkFilterRuleId;
use rd_db::StoreErrorKind;

pub use crate::link_filter_input::{
    CandidateUnhideRequest, CandidateUnhideResponse, LinkFilterApplyResponse,
    LinkFilterReorderRequest, LinkFilterRuleRequest, validated_link_filter_rule,
};
use crate::{ApiError, AppState, dto::MessageResponse};

fn not_found(error: &anyhow::Error) -> ApiError {
    crate::error_codes::store_error(
        error,
        "link_filter.not_found",
        "LinkFilter rule not found",
        StoreErrorKind::InUse,
        "link_filter.in_use",
        "The LinkFilter rule is still in use",
    )
}

#[utoipa::path(get, path = "/api/v1/link-filters", tag = "collector", responses((status = 200, body = [rd_core::LinkFilterRule])))]
pub async fn list_link_filter_rules(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_core::LinkFilterRule>>, ApiError> {
    Ok(Json(state.database.list_link_filter_rules().await?))
}

#[utoipa::path(post, path = "/api/v1/link-filters", tag = "collector", request_body = LinkFilterRuleRequest, responses((status = 201, body = rd_core::LinkFilterRule), (status = 400, body = crate::error::ErrorBody)))]
pub async fn create_link_filter_rule(
    State(state): State<AppState>,
    Json(request): Json<LinkFilterRuleRequest>,
) -> Result<(StatusCode, Json<rd_core::LinkFilterRule>), ApiError> {
    let input = validated_link_filter_rule(&state, request).await?;
    let rule = state.database.create_link_filter_rule(input).await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

#[utoipa::path(put, path = "/api/v1/link-filters/{id}", tag = "collector", params(("id" = rd_core::LinkFilterRuleId, Path)), request_body = LinkFilterRuleRequest, responses((status = 200, body = rd_core::LinkFilterRule), (status = 400, body = crate::error::ErrorBody), (status = 404)))]
pub async fn update_link_filter_rule(
    State(state): State<AppState>,
    Path(id): Path<LinkFilterRuleId>,
    Json(request): Json<LinkFilterRuleRequest>,
) -> Result<Json<rd_core::LinkFilterRule>, ApiError> {
    let input = validated_link_filter_rule(&state, request).await?;
    let rule = state
        .database
        .update_link_filter_rule(id, input)
        .await
        .map_err(|error| not_found(&error))?;
    Ok(Json(rule))
}

/// Deletes a rule; the links it hid are shown again, nothing else changes.
#[utoipa::path(delete, path = "/api/v1/link-filters/{id}", tag = "collector", params(("id" = rd_core::LinkFilterRuleId, Path)), responses((status = 200, body = MessageResponse), (status = 404)))]
pub async fn delete_link_filter_rule(
    State(state): State<AppState>,
    Path(id): Path<LinkFilterRuleId>,
) -> Result<Json<MessageResponse>, ApiError> {
    state
        .database
        .delete_link_filter_rule(id)
        .await
        .map_err(|error| not_found(&error))?;
    Ok(Json(MessageResponse::new(
        "link_filter.deleted",
        "LinkFilter rule deleted",
    )))
}

/// Puts the named rules first, in the order given; the first matching rule decides.
#[utoipa::path(post, path = "/api/v1/link-filters/reorder", tag = "collector", request_body = LinkFilterReorderRequest, responses((status = 200, body = [rd_core::LinkFilterRule]), (status = 400, body = crate::error::ErrorBody)))]
pub async fn reorder_link_filter_rules(
    State(state): State<AppState>,
    Json(request): Json<LinkFilterReorderRequest>,
) -> Result<Json<Vec<rd_core::LinkFilterRule>>, ApiError> {
    rd_api_core::list_bounds::validate_bulk(request.ids.len())?;
    state
        .database
        .reorder_link_filter_rules(request.ids)
        .await?;
    Ok(Json(state.database.list_link_filter_rules().await?))
}

/// Decides every link in the LinkGrabber anew by the rules as they are now.
///
/// Hidden links are kept, never deleted, and a link already in the downloads is not touched.
#[utoipa::path(post, path = "/api/v1/link-filters/apply", tag = "collector", responses((status = 200, body = LinkFilterApplyResponse)))]
pub async fn apply_link_filters(
    State(state): State<AppState>,
) -> Result<Json<LinkFilterApplyResponse>, ApiError> {
    Ok(Json(state.database.apply_link_filters().await?.into()))
}

/// Shows links a LinkFilter rule hid, until the rules are applied again.
#[utoipa::path(post, path = "/api/v1/collector/candidates/unhide", tag = "collector", request_body = CandidateUnhideRequest, responses((status = 200, body = CandidateUnhideResponse), (status = 400, body = crate::error::ErrorBody)))]
pub async fn unhide_candidates(
    State(state): State<AppState>,
    Json(request): Json<CandidateUnhideRequest>,
) -> Result<Json<CandidateUnhideResponse>, ApiError> {
    rd_api_core::list_bounds::validate_bulk(request.candidate_ids.len())?;
    let shown = state
        .database
        .show_filtered_candidates(request.candidate_ids)
        .await?;
    Ok(Json(CandidateUnhideResponse { shown }))
}
