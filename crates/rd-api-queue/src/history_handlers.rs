//! The download history (RD-1100-04): what was downloaded, searchable after the package left
//! the queue.
//!
//! Paged in SQL rather than sliced here like the other API-15 lists: the history grows with
//! every finished package and is read with a search, so the database counts and cuts it.

use axum::{Json, extract::State, http::HeaderMap};
use chrono::{DateTime, Utc};
use rd_api_core::list_bounds::{Page, total_header};
use serde::Deserialize;
use utoipa::IntoParams;

use crate::{ApiError, AppState, dto::PageQuery};

/// The filters of `GET /api/v1/history`; every one is optional.
#[derive(Clone, Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HistoryFilterQuery {
    /// Part of the name or of a source address, case-insensitive.
    pub q: Option<String>,
    /// `completed` or `failed`.
    pub outcome: Option<rd_core::HistoryOutcome>,
    /// The download kind (`http`, `usenet`, `torrent`, ...).
    pub kind: Option<rd_core::DownloadKind>,
    /// Entries that ended at or after this instant (RFC 3339).
    pub from: Option<DateTime<Utc>>,
    /// Entries that ended at or before this instant (RFC 3339).
    pub to: Option<DateTime<Utc>>,
}

/// `400` for a filter value that does not parse, with a stable code like every REST refusal.
#[must_use]
pub fn history_filter_invalid() -> ApiError {
    ApiError::bad_request(
        "history.filter_invalid",
        "The history filter is not valid: outcome, kind or the time range does not parse",
    )
}

/// The filters a request asks for, read from its query string beside `limit` and `offset`.
///
/// An extractor of its own for the reason [`Page`] is one: axum answers a value that does not
/// parse with an uncoded plain-text `400`.
#[derive(Clone, Debug, Default)]
pub struct HistoryFilter(pub HistoryFilterQuery);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for HistoryFilter {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        axum::extract::Query::<HistoryFilterQuery>::try_from_uri(&parts.uri)
            .map(|axum::extract::Query(query)| Self(query))
            .map_err(|_| history_filter_invalid())
    }
}

/// The history, newest first, filtered and paged; `limit`/`offset` cut the page in the
/// database and `X-Total-Count` names how many entries the filters match.
#[utoipa::path(
    get,
    path = "/api/v1/history",
    tag = "history",
    params(PageQuery, HistoryFilterQuery),
    responses(
        (status = 200, body = [rd_core::HistoryEntry], headers(("x-total-count" = u64, description = "How many entries the filters match; sent only when `limit` or `offset` asked for a page"))),
        (status = 400, description = "request.page_limit or history.filter_invalid"),
    )
)]
pub async fn list_download_history(
    State(state): State<AppState>,
    Page(page): Page,
    HistoryFilter(filter): HistoryFilter,
) -> Result<(HeaderMap, Json<Vec<rd_core::HistoryEntry>>), ApiError> {
    let window = page.window()?;
    let query = rd_db::HistoryQuery {
        search: filter.q,
        outcome: filter.outcome,
        kind: filter.kind,
        finished_from: filter.from,
        finished_to: filter.to,
        compat_visible_only: false,
        offset: window.map_or(0, |window| u64::try_from(window.offset).unwrap_or(u64::MAX)),
        limit: window.and_then(|window| u64::try_from(window.limit).ok()),
    };
    let result = state.database.list_download_history(&query).await?;
    Ok((total_header(window, result.total), Json(result.entries)))
}
