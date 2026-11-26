//! `GET <collection>/{id}` of the sources and the LinkGrabber (RD-1480-10, API-6): one row as its
//! list shows it.
//!
//! Every handler keeps one row of its list, or reads the row alone where the list grows, so the
//! two cannot drift apart in shape or in what they reveal (`rd_api_core::single_read`). The route
//! costs what the list costs.

use axum::extract::State;
use rd_api_core::extract::{Json, Path};
use rd_api_core::single_read::one_of;

use crate::{ApiError, AppState, error_codes::parse_id};

/// One indexer, without its API key.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/indexers/{id}", tag = "indexers", params(("id" = rd_core::IndexerId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::Indexer), (status = 404)))]
pub async fn get_indexer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::Indexer>, ApiError> {
    let id = parse_id::<rd_core::IndexerId>(&id)?;
    let Json(rows) = crate::indexer_handlers::list_indexers(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("indexer.not_found", "Indexer not found"),
    )
}

/// One link filter rule.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/link-filters/{id}", tag = "collector", params(("id" = rd_core::LinkFilterRuleId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::LinkFilterRule), (status = 404)))]
pub async fn get_link_filter_rule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::LinkFilterRule>, ApiError> {
    let id = parse_id::<rd_core::LinkFilterRuleId>(&id)?;
    let Json(rows) = crate::link_filter_handlers::list_link_filter_rules(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("link_filter.not_found", "LinkFilter rule not found"),
    )
}

/// One watched livestream channel.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/streams/channels/{id}", tag = "streams", params(("id" = rd_core::StreamChannelId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::StreamChannel), (status = 404)))]
pub async fn get_stream_channel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::StreamChannel>, ApiError> {
    let id = parse_id::<rd_core::StreamChannelId>(&id)?;
    let Json(rows) = crate::stream_handlers::list_stream_channels(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("stream.channel_not_found", "Stream channel not found"),
    )
}

/// One recording schedule.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/streams/schedules/{id}", tag = "streams", params(("id" = rd_core::StreamScheduleId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::StreamSchedule), (status = 404)))]
pub async fn get_stream_schedule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::StreamSchedule>, ApiError> {
    let id = parse_id::<rd_core::StreamScheduleId>(&id)?;
    let Json(rows) = crate::stream_schedule_handlers::list_stream_schedules(State(state)).await?;
    one_of(
        rows,
        |row| row.id == id,
        || ApiError::not_found("stream.schedule_not_found", "Schedule not found"),
    )
}

/// One subscription.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/subscriptions/{id}", tag = "subscriptions", params(("id" = rd_core::SubscriptionId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::Subscription), (status = 404)))]
pub async fn get_subscription(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::Subscription>, ApiError> {
    state
        .database
        .subscription(parse_id(&id)?)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("subscription.not_found", "Subscription not found"))
}

/// One site rule with its switch, its origin and its last check.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/site-rules/{id}", tag = "configuration", params(("id" = String, Path, description = "The id the list shows")), responses((status = 200, body = crate::site_rules_dto::SiteRuleResponse), (status = 404)))]
pub async fn get_site_rule(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<crate::site_rules_dto::SiteRuleResponse>, ApiError> {
    let Json(list) = crate::site_rules_handlers::list_site_rules(State(state)).await?;
    one_of(
        list.rules,
        |rule| rule.id == id,
        || {
            ApiError::not_found("site_rules.not_found", "No rule with that id")
                .with_param("rule", &id)
        },
    )
}

/// One link of the LinkGrabber. A link already queued is gone from here, as it is from the list.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/collector/candidates/{id}", tag = "collector", params(("id" = rd_core::CandidateId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::LinkCandidate), (status = 404)))]
pub async fn get_candidate(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::LinkCandidate>, ApiError> {
    state
        .database
        .get_candidate(parse_id(&id)?)
        .await?
        .filter(|candidate| candidate.state != rd_core::LinkCandidateState::Enqueued)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("collector.candidate_not_found", "Link not found"))
}

/// One package of the LinkGrabber. One whose links are all queued is gone from here, as it is
/// from the list.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/collector/packages/{id}", tag = "collector", params(("id" = rd_core::CollectorPackageId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::CollectorPackage), (status = 404)))]
pub async fn get_collector_package(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::CollectorPackage>, ApiError> {
    state
        .database
        .open_collector_package(parse_id(&id)?)
        .await?
        .map(Json)
        .ok_or_else(crate::error_codes::package_not_found)
}

/// One imported NZB.
///
/// The row as its list shows it, at the list's permission; an unknown id is a `404` with a code.
#[utoipa::path(get, path = "/api/v1/nzb/imports/{id}", tag = "collector", params(("id" = rd_core::NzbImportId, Path, description = "The id the list shows")), responses((status = 200, body = rd_core::NzbImport), (status = 404)))]
pub async fn get_nzb_import(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<rd_core::NzbImport>, ApiError> {
    state
        .database
        .get_nzb_import(parse_id(&id)?)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("nzb.import_not_found", "NZB import not found"))
}
