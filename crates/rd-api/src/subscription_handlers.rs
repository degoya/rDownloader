//! REST surface for subscriptions (RD-080-07).
//!
//! Validation happens here rather than in the store, following the `stream_handlers`
//! precedent: the database is handed an already-clean `NewSubscription`, and every refusal
//! carries a stable code the UI translates.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use rd_core::{
    BacklogPolicy, DownloadPriority, MAX_FILTER_PATTERNS, MAX_POLL_INTERVAL_SECONDS, Subscription,
    SubscriptionBulkStateResponse, SubscriptionFilters, SubscriptionHistoryClearResponse,
    SubscriptionId, SubscriptionItem, SubscriptionItemId, SubscriptionItemPage,
    SubscriptionItemState, SubscriptionKind, SubscriptionMode, SubscriptionReviewSummary,
    SubscriptionRun,
};
use rd_db::NewSubscription;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{AppState, error::ApiError};

/// Longest name and URL accepted.
const MAX_NAME: usize = 200;
const MAX_URL: usize = 2_000;
/// Most items and runs one response returns.
const PAGE_LIMIT: i64 = 200;
const DEFAULT_ITEM_PAGE_LIMIT: i64 = 50;

/// Create or replace one subscription.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SubscriptionRequest {
    pub name: String,
    #[schema(format = "uri")]
    pub url: String,
    pub kind: SubscriptionKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: SubscriptionMode,
    #[serde(default)]
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default)]
    pub priority: DownloadPriority,
    #[serde(default = "default_interval")]
    pub interval_seconds: u32,
    #[serde(default)]
    pub filters: SubscriptionFilters,
    #[serde(default)]
    pub backlog: BacklogPolicy,
    /// Indexer categories routed to categories of ours (RD-080-11).
    #[serde(default)]
    pub category_map: Vec<rd_core::CategoryMapping>,
    /// Indexer categories to ask for. Empty asks for everything, as before.
    #[serde(default)]
    pub source_categories: Vec<String>,
    /// Keep every release of an episode rather than only the first (RD-110-21). Only a
    /// watched release page reads it; for every other kind identity is the address anyway.
    #[serde(default)]
    pub every_release: bool,
    /// How the LinkGrabber draws the pending hits: `list` (the default) or `cards`
    /// (RD-120-37).
    #[serde(default)]
    pub view: rd_core::SubscriptionView,
    /// Whether the card slider turns its pages on its own; off unless asked for, and ignored
    /// by the list (RD-120-37).
    #[serde(default)]
    pub autoplay: bool,
    /// The shape of a card's image area in the card view: `1:1`, `3:2`, `16:9`, `4:3` or
    /// `2:1` (the default) (RD-120-42). Anything else is refused with
    /// `subscription.card_ratio_unknown` rather than drawn as the default.
    #[serde(default = "default_card_ratio")]
    #[schema(value_type = rd_core::SubscriptionCardRatio)]
    pub card_ratio: String,
    /// A cron expression that replaces the interval: five fields in the service's local time,
    /// e.g. `0 6 * * *` for six every morning (RD-130-19). Only a `script` subscription takes
    /// one; empty or absent keeps the interval.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Indexer API key (RD-080-11); write-only, and stored in the vault. Omitting it on an
    /// edit keeps the existing key rather than clearing it.
    #[serde(default)]
    #[schema(write_only)]
    pub api_key: Option<String>,
}

const fn default_true() -> bool {
    true
}

/// Read as a string rather than as the enum, so an unknown ratio reaches
/// [`subscription_input`] and is refused there with a stable code, over REST and MCP alike.
fn default_card_ratio() -> String {
    rd_core::SubscriptionCardRatio::default()
        .as_str()
        .to_owned()
}

const fn default_interval() -> u32 {
    rd_core::DEFAULT_POLL_INTERVAL_SECONDS
}

/// How many rows a listing returns.
#[derive(Debug, Deserialize, IntoParams)]
pub struct PageQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

impl PageQuery {
    fn limit(&self) -> i64 {
        self.limit.unwrap_or(PAGE_LIMIT).clamp(1, PAGE_LIMIT)
    }
}

/// Server-side item archive filter.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionItemFilter {
    #[default]
    Pending,
    Queued,
    Dismissed,
    Skipped,
    All,
}

/// Pagination and state selection for the item archive.
#[derive(Debug, Deserialize, IntoParams)]
pub struct SubscriptionItemPageQuery {
    #[serde(default)]
    pub state: SubscriptionItemFilter,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

impl SubscriptionItemPageQuery {
    fn state(&self) -> Option<SubscriptionItemState> {
        match self.state {
            SubscriptionItemFilter::Pending => Some(SubscriptionItemState::Pending),
            SubscriptionItemFilter::Queued => Some(SubscriptionItemState::Queued),
            SubscriptionItemFilter::Dismissed => Some(SubscriptionItemState::Dismissed),
            SubscriptionItemFilter::Skipped => Some(SubscriptionItemState::Skipped),
            SubscriptionItemFilter::All => None,
        }
    }

    fn limit(&self) -> i64 {
        self.limit
            .unwrap_or(DEFAULT_ITEM_PAGE_LIMIT)
            .clamp(1, PAGE_LIMIT)
    }

    fn offset(&self) -> i64 {
        self.offset.unwrap_or_default().max(0)
    }
}

/// Longest script name, the same bound the scripts directory's resolver applies.
const MAX_SCRIPT_NAME: usize = 128;

/// The `script:<name>` address of a script subscription (RD-130-19).
///
/// A bare name is accepted as well, because it is what somebody types. The name follows the
/// scripts directory's own rule -- letters, digits, `.`, `_`, `-`, not starting with a dot --
/// so a subscription can only ever point at a file directly inside that directory.
fn script_url(raw: &str) -> Result<url::Url, ApiError> {
    let name = raw
        .strip_prefix(rd_core::SCRIPT_URL_SCHEME)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(raw);
    let valid = !name.is_empty()
        && name.len() <= MAX_SCRIPT_NAME
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        return Err(ApiError::unprocessable(
            "subscription.script_name_invalid",
            "A script is named by a file directly inside the scripts directory",
        )
        .with_param("value", name.chars().take(64).collect::<String>()));
    }
    url::Url::parse(&format!("{}:{name}", rd_core::SCRIPT_URL_SCHEME)).map_err(|_| {
        ApiError::unprocessable(
            "subscription.script_name_invalid",
            "A script is named by a file directly inside the scripts directory",
        )
        .with_param("value", name.to_owned())
    })
}

/// The cron expression, checked to name a time (RD-130-19); `None` keeps the interval.
fn schedule_input(request: &SubscriptionRequest) -> Result<Option<String>, ApiError> {
    let Some(expression) = request
        .schedule
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    // Script subscriptions only, for now: every other kind polls somebody else's server, and
    // `* * * * *` would walk straight past the per-kind interval floor that protects it.
    if request.kind != SubscriptionKind::Script {
        return Err(ApiError::unprocessable(
            "subscription.schedule_kind",
            "Only a script subscription runs on a schedule",
        ));
    }
    rd_subscription::next_scheduled(expression, chrono::Utc::now(), &chrono::Local).map_err(
        |error| {
            ApiError::unprocessable("subscription.schedule_invalid", error.to_string())
                .with_param("value", expression.chars().take(64).collect::<String>())
        },
    )?;
    Ok(Some(expression.to_owned()))
}

/// A script subscription starts code on this machine, so creating one, changing one, or
/// turning one into something else costs the administration scope (RD-130-19) -- whatever
/// the route itself costs. `kinds` are the kinds involved: the requested one, and on an edit
/// the stored one.
fn require_admin_for_script(
    granted: Option<&crate::auth::Granted>,
    kinds: &[SubscriptionKind],
) -> Result<(), ApiError> {
    if !kinds.contains(&SubscriptionKind::Script)
        || granted.is_some_and(|granted| granted.holds(rd_core::Scope::Admin))
    {
        return Ok(());
    }
    Err(ApiError::forbidden(
        "auth.scope_insufficient",
        "A script subscription requires the administration scope",
    )
    .with_param("scope", rd_core::Scope::Admin.as_str()))
}

/// The same rule for an action on a stored subscription: switching a script subscription on
/// or off decides when code runs, and "check now" runs it (RD-130-19). An unknown id passes,
/// so the action itself answers with its own 404.
async fn require_admin_for_stored_script(
    state: &AppState,
    granted: Option<&crate::auth::Granted>,
    id: SubscriptionId,
) -> Result<(), ApiError> {
    let stored = state
        .database
        .subscription(id)
        .await?
        .map(|subscription| subscription.kind);
    require_admin_for_script(granted, &stored.into_iter().collect::<Vec<_>>())
}

/// Refuses a script subscription whose script is not in the scripts directory.
///
/// Checked when it is saved rather than only when it runs, so a typo is a form error and not
/// a failure in the history the next morning. The run checks again, because a file can go.
async fn ensure_script_exists(state: &AppState, input: &NewSubscription) -> Result<(), ApiError> {
    if input.kind != SubscriptionKind::Script {
        return Ok(());
    }
    let name = input.url.path();
    let directory = state.extraction.scripts_directory().await?;
    if tokio::fs::metadata(directory.join(name))
        .await
        .is_ok_and(|meta| meta.is_file())
    {
        return Ok(());
    }
    Err(ApiError::unprocessable(
        "subscription.script_not_found",
        "There is no such script in the scripts directory",
    )
    .with_param("name", name.to_owned()))
}

/// Validates the request and turns it into what the store accepts.
///
/// `api_key` is minted into the vault by the caller, because a failed insert has to remove
/// the reference again and only the caller knows whether the insert succeeded.
pub(crate) fn subscription_input(
    request: &SubscriptionRequest,
    secret_ref: Option<String>,
) -> Result<NewSubscription, ApiError> {
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(ApiError::bad_request(
            "subscription.name_invalid",
            "A subscription needs a name",
        ));
    }
    let raw = request.url.trim();
    if raw.is_empty() || raw.len() > MAX_URL {
        return Err(ApiError::bad_request(
            "subscription.url_invalid",
            "A subscription needs an address",
        ));
    }
    let url = if request.kind == SubscriptionKind::Script {
        script_url(raw)?
    } else {
        let url = url::Url::parse(raw).map_err(|_| {
            ApiError::bad_request("subscription.url_invalid", "Address is not a URL")
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ApiError::bad_request(
                "subscription.url_scheme",
                "Only http and https addresses can be polled",
            ));
        }
        url
    };
    let schedule = schedule_input(request)?;
    // Refused rather than clamped: a person who typed 30 seconds should be told the limit,
    // not silently given something twenty times slower than they asked for. The floor is the
    // kind's (RD-110-21), because a board page is not an indexer -- `effective_interval`
    // clamps to the same number for anything that reaches the poller by another road.
    let minimum = request.kind.min_interval_seconds();
    if !(minimum..=MAX_POLL_INTERVAL_SECONDS).contains(&request.interval_seconds) {
        return Err(ApiError::unprocessable(
            "subscription.interval_invalid",
            "Poll interval is outside the permitted range",
        )
        .with_param("minimum", minimum.to_string())
        .with_param("maximum", MAX_POLL_INTERVAL_SECONDS.to_string()));
    }
    let filters = sanitize_filters(&request.filters)?;
    // Refused rather than defaulted: somebody who sent `21:9` asked for something, and
    // quietly drawing `2:1` instead would look like the setting did not take.
    let card_ratio =
        rd_core::SubscriptionCardRatio::parse(&request.card_ratio).ok_or_else(|| {
            ApiError::unprocessable(
                "subscription.card_ratio_unknown",
                "Card ratio is not one of 1:1, 3:2, 16:9, 4:3, 2:1",
            )
            .with_param(
                "value",
                request.card_ratio.chars().take(16).collect::<String>(),
            )
        })?;
    Ok(NewSubscription {
        source_categories: sanitize_source_categories(&request.source_categories)?,
        name: name.to_owned(),
        url,
        kind: request.kind,
        enabled: request.enabled,
        mode: request.mode,
        category_id: request.category_id,
        priority: request.priority,
        interval_seconds: request.interval_seconds,
        filters,
        backlog: request.backlog,
        category_map: sanitize_category_map(&request.category_map)?,
        every_release: request.every_release,
        view: request.view,
        autoplay: request.autoplay,
        card_ratio,
        schedule,
        secret_ref,
    })
}

/// Bounds the category map and drops entries with an empty source category.
///
/// A duplicate source category is kept as written rather than merged: the lookup takes the
/// first match, and silently discarding the second would hide a mistake the user can see.
/// Trims the requested categories, drops empties and duplicates, and caps the list.
///
/// Duplicates go here but deliberately not in `sanitize_category_map`: a repeated mapping is the
/// user's business, while a repeated `cat` value would just be sent twice to the indexer.
pub(crate) fn sanitize_source_categories(categories: &[String]) -> Result<Vec<String>, ApiError> {
    let mut cleaned: Vec<String> = Vec::new();
    for value in categories {
        let value = value.trim();
        if value.is_empty() || cleaned.iter().any(|kept| kept == value) {
            continue;
        }
        cleaned.push(value.to_owned());
    }
    if cleaned.len() > rd_core::MAX_CATEGORY_MAPPINGS {
        return Err(ApiError::unprocessable(
            "subscription.source_categories_too_many",
            "Too many indexer categories",
        )
        .with_param("maximum", rd_core::MAX_CATEGORY_MAPPINGS.to_string()));
    }
    Ok(cleaned)
}

pub(crate) fn sanitize_category_map(
    mappings: &[rd_core::CategoryMapping],
) -> Result<Vec<rd_core::CategoryMapping>, ApiError> {
    let cleaned: Vec<rd_core::CategoryMapping> = mappings
        .iter()
        .filter(|mapping| !mapping.source_category.trim().is_empty())
        .map(|mapping| rd_core::CategoryMapping {
            source_category: mapping.source_category.trim().to_owned(),
            category_id: mapping.category_id,
        })
        .collect();
    if cleaned.len() > rd_core::MAX_CATEGORY_MAPPINGS {
        return Err(ApiError::unprocessable(
            "subscription.category_map_too_many",
            "Too many category mappings",
        )
        .with_param("maximum", rd_core::MAX_CATEGORY_MAPPINGS.to_string()));
    }
    Ok(cleaned)
}

/// Trims and bounds the pattern lists, and refuses a range that can match nothing.
fn sanitize_filters(filters: &SubscriptionFilters) -> Result<SubscriptionFilters, ApiError> {
    let clean = |values: &[String]| -> Result<Vec<String>, ApiError> {
        let cleaned: Vec<String> = values
            .iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect();
        if cleaned.len() > MAX_FILTER_PATTERNS {
            return Err(ApiError::unprocessable(
                "subscription.filters_too_many",
                "Too many filter patterns",
            )
            .with_param("maximum", MAX_FILTER_PATTERNS.to_string()));
        }
        Ok(cleaned)
    };
    // An inverted range accepts nothing, and a subscription that silently accepts nothing is
    // exactly the failure this feature is supposed to make visible.
    if let (Some(minimum), Some(maximum)) =
        (filters.min_duration_seconds, filters.max_duration_seconds)
        && minimum > maximum
    {
        return Err(ApiError::unprocessable(
            "subscription.duration_range_invalid",
            "The shortest duration is longer than the longest",
        ));
    }
    Ok(SubscriptionFilters {
        title_contains: clean(&filters.title_contains)?,
        title_excludes: clean(&filters.title_excludes)?,
        languages: clean(&filters.languages)?,
        ..filters.clone()
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/subscriptions",
    tag = "subscriptions",
    responses((status = 200, body = Vec<Subscription>))
)]
pub async fn list_subscriptions(
    State(state): State<AppState>,
) -> Result<Json<Vec<Subscription>>, ApiError> {
    Ok(Json(state.database.list_subscriptions().await?))
}

#[utoipa::path(
    post,
    path = "/api/v1/subscriptions",
    tag = "subscriptions",
    request_body = SubscriptionRequest,
    responses(
        (status = 201, body = Subscription),
        (status = 400, body = crate::error::ErrorBody),
        (status = 403, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn create_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Json(request): Json<SubscriptionRequest>,
) -> Result<(StatusCode, Json<Subscription>), ApiError> {
    require_admin_for_script(
        granted.as_ref().map(|axum::Extension(granted)| granted),
        &[request.kind],
    )?;
    // Validated before the key is minted, so a rejected request leaves nothing behind.
    ensure_script_exists(&state, &subscription_input(&request, None)?).await?;
    let secret_ref =
        crate::config_handlers::store_optional(&state.secrets, request.api_key.clone()).await?;
    let input = subscription_input(&request, secret_ref.clone())?;
    match state.database.create_subscription(input).await {
        Ok(created) => Ok((StatusCode::CREATED, Json(created))),
        Err(error) => {
            // A reference nothing points at would never be cleaned up.
            crate::config_handlers::cleanup_secrets(&state.secrets, [secret_ref]).await;
            Err(error.into())
        }
    }
}

#[utoipa::path(
    put,
    path = "/api/v1/subscriptions/{id}",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    request_body = SubscriptionRequest,
    responses(
        (status = 200, body = Subscription),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn update_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Path(id): Path<SubscriptionId>,
    Json(request): Json<SubscriptionRequest>,
) -> Result<Json<Subscription>, ApiError> {
    // The stored kind counts too: turning a script into a feed changes a script subscription.
    let stored = state
        .database
        .subscription(id)
        .await?
        .map(|subscription| subscription.kind);
    require_admin_for_script(
        granted.as_ref().map(|axum::Extension(granted)| granted),
        &[Some(request.kind), stored]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
    )?;
    ensure_script_exists(&state, &subscription_input(&request, None)?).await?;
    let minted =
        crate::config_handlers::store_optional(&state.secrets, request.api_key.clone()).await?;
    let input = subscription_input(&request, minted.clone())?;
    match state.database.update_subscription(id, input).await {
        Ok((updated, orphan)) => {
            // Only the reference this edit replaced is dropped; an unchanged key survives a
            // form that did not resend it.
            crate::config_handlers::cleanup_secrets(&state.secrets, [orphan]).await;
            Ok(Json(updated))
        }
        Err(error) => {
            crate::config_handlers::cleanup_secrets(&state.secrets, [minted]).await;
            Err(not_found(error))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/{id}/enable",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 200, body = Subscription),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn enable_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<Subscription>, ApiError> {
    require_admin_for_stored_script(
        &state,
        granted.as_ref().map(|axum::Extension(granted)| granted),
        id,
    )
    .await?;
    state
        .database
        .set_subscription_enabled(id, true)
        .await
        .map(Json)
        .map_err(not_found)
}

#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/{id}/disable",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 200, body = Subscription),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn disable_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<Subscription>, ApiError> {
    require_admin_for_stored_script(
        &state,
        granted.as_ref().map(|axum::Extension(granted)| granted),
        id,
    )
    .await?;
    state
        .database
        .set_subscription_enabled(id, false)
        .await
        .map(Json)
        .map_err(not_found)
}

#[utoipa::path(
    delete,
    path = "/api/v1/subscriptions/{id}",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses((status = 204), (status = 404, body = crate::error::ErrorBody))
)]
pub async fn delete_subscription(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
) -> Result<StatusCode, ApiError> {
    let secret_ref = state
        .database
        .delete_subscription(id)
        .await
        .map_err(not_found)?;
    crate::config_handlers::cleanup_secrets(&state.secrets, [secret_ref]).await;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/subscriptions/{id}/items",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path), PageQuery),
    responses((status = 200, body = Vec<SubscriptionItem>))
)]
pub async fn list_subscription_items(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<SubscriptionItem>>, ApiError> {
    Ok(Json(
        state.database.subscription_items(id, page.limit()).await?,
    ))
}

/// Lists one state-filtered page without changing the legacy array-shaped item endpoint.
#[utoipa::path(
    get,
    path = "/api/v1/subscriptions/{id}/items/page",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path), SubscriptionItemPageQuery),
    responses(
        (status = 200, body = SubscriptionItemPage),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn list_subscription_item_page(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
    Query(page): Query<SubscriptionItemPageQuery>,
) -> Result<Json<SubscriptionItemPage>, ApiError> {
    if state.database.subscription(id).await?.is_none() {
        return Err(ApiError::not_found(
            "subscription.not_found",
            "Subscription not found",
        ));
    }
    Ok(Json(
        state
            .database
            .subscription_item_page(id, page.state(), page.limit(), page.offset())
            .await?,
    ))
}

/// Pending review counts for all configured indexer subscriptions.
#[utoipa::path(
    get,
    path = "/api/v1/subscriptions/review-summary",
    tag = "subscriptions",
    responses((status = 200, body = SubscriptionReviewSummary))
)]
pub async fn subscription_review_summary(
    State(state): State<AppState>,
) -> Result<Json<SubscriptionReviewSummary>, ApiError> {
    Ok(Json(state.database.subscription_review_summary().await?))
}

#[utoipa::path(
    get,
    path = "/api/v1/subscriptions/{id}/runs",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path), PageQuery),
    responses((status = 200, body = Vec<SubscriptionRun>))
)]
pub async fn list_subscription_runs(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<SubscriptionRun>>, ApiError> {
    Ok(Json(
        state.database.subscription_runs(id, page.limit()).await?,
    ))
}

/// Polls one subscription immediately, outside its schedule.
#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/{id}/poll",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 202),
        (status = 403, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn poll_subscription(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Path(id): Path<SubscriptionId>,
) -> Result<StatusCode, ApiError> {
    let Some(subscription) = state.database.subscription(id).await? else {
        return Err(ApiError::not_found(
            "subscription.not_found",
            "Subscription not found",
        ));
    };
    // "Check now" on a script subscription runs the script: the route costs `api:queue`, the
    // script costs `api:admin` (RD-130-19).
    require_admin_for_script(
        granted.as_ref().map(|axum::Extension(granted)| granted),
        &[subscription.kind],
    )?;
    // Spawned rather than awaited: a poll contacts somebody else's server and can take the
    // configured timeout, which is far longer than a request should hold a connection.
    let service = state.subscriptions.clone();
    tokio::spawn(async move {
        if let Err(error) = service.poll_now(id).await {
            tracing::warn!(%error, "manual subscription poll failed");
        }
    });
    Ok(StatusCode::ACCEPTED)
}

/// Body of the item-state change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SubscriptionItemStateRequest {
    pub state: SubscriptionItemState,
}

/// Sets every item that was pending when this request started.
#[utoipa::path(
    put,
    path = "/api/v1/subscriptions/{id}/items/pending",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    request_body = SubscriptionItemStateRequest,
    responses(
        (status = 200, body = SubscriptionBulkStateResponse),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody)
    )
)]
pub async fn set_pending_subscription_items_state(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
    Json(request): Json<SubscriptionItemStateRequest>,
) -> Result<Json<SubscriptionBulkStateResponse>, ApiError> {
    if state.database.subscription(id).await?.is_none() {
        return Err(ApiError::not_found(
            "subscription.not_found",
            "Subscription not found",
        ));
    }
    if !matches!(
        request.state,
        SubscriptionItemState::Queued | SubscriptionItemState::Dismissed
    ) {
        return Err(ApiError::unprocessable(
            "subscription.bulk_state_invalid",
            "Pending items can only be queued or dismissed",
        ));
    }
    let ids = state.database.pending_subscription_item_ids(id).await?;
    let matched = u64::try_from(ids.len()).unwrap_or(u64::MAX);
    let updated = if request.state == SubscriptionItemState::Dismissed {
        state
            .database
            .set_pending_subscription_items_state(ids, request.state)
            .await?
    } else {
        let mut ready = Vec::with_capacity(ids.len());
        for item_id in ids {
            if queue_reviewed_item(&state, item_id).await.is_ok() {
                ready.push(item_id);
            }
        }
        state
            .database
            .set_pending_subscription_items_state(ready, request.state)
            .await?
    };
    Ok(Json(SubscriptionBulkStateResponse {
        matched,
        updated,
        failed: matched.saturating_sub(updated),
    }))
}

/// Removes settled items and poll history while preserving everything still awaiting review.
#[utoipa::path(
    delete,
    path = "/api/v1/subscriptions/{id}/history",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 200, body = SubscriptionHistoryClearResponse),
        (status = 404, body = crate::error::ErrorBody)
    )
)]
pub async fn clear_subscription_history(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<SubscriptionHistoryClearResponse>, ApiError> {
    state
        .database
        .clear_subscription_history(id)
        .await
        .map(Json)
        .map_err(not_found)
}

/// Queues or dismisses one reviewed item.
///
/// Queueing hands the item to the LinkGrabber on the way, through the same intake the automatic
/// poll uses. The state alone would only move a row in the subscription's own table, which is
/// invisible everywhere else in the application.
#[utoipa::path(
    put,
    path = "/api/v1/subscriptions/items/{id}",
    tag = "subscriptions",
    params(("id" = SubscriptionItemId, Path)),
    request_body = SubscriptionItemStateRequest,
    responses(
        (status = 204),
        (status = 404, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody)
    )
)]
pub async fn set_subscription_item_state(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionItemId>,
    Json(request): Json<SubscriptionItemStateRequest>,
) -> Result<StatusCode, ApiError> {
    if request.state == rd_core::SubscriptionItemState::Queued {
        queue_reviewed_item(&state, id).await?;
    }
    state
        .database
        .set_subscription_item_state(id, request.state)
        .await
        .map_err(not_found)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Hands one reviewed item to the intake before its state is written.
///
/// Intake first, state second: an item that could not be handed over keeps its pending state,
/// so the review list still shows it and the action can be repeated. The reverse order would
/// leave a row claiming to be queued with nothing behind it.
async fn queue_reviewed_item(state: &AppState, id: SubscriptionItemId) -> Result<(), ApiError> {
    let item = state
        .database
        .subscription_item(id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| {
            ApiError::not_found("subscription.item_not_found", "Subscription item not found")
        })?;
    let subscription = state
        .database
        .subscription(item.subscription_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found("subscription.not_found", "Subscription not found"))?;
    let category_id = subscription.category_for(item.source_category.as_deref());
    let intake = crate::subscription_service::SubscriptionIntake {
        database: &state.database,
        link_check: &state.link_check,
        media_settings: &state.media_settings,
        gallery_settings: &state.gallery_settings,
    };
    crate::subscription_service::hand_urls_to_intake(
        &intake,
        &subscription.name,
        vec![crate::collector_handlers::DeclaredLink {
            url: item.url.clone(),
            media_type: item.media_type.clone(),
            name: Some(rd_files::strip_password_marker(&item.title).0),
            // Read back from the archived row, so a hit queued after review carries the same
            // password a hit queued automatically would.
            password: item.password.clone(),
            // And the same declared attributes, through the same gate: a hit queued after
            // review must reach an enricher with exactly what an auto-queued one reaches it
            // with (RD-107-02).
            attributes: rd_subscription::retain_attributes(&item.attributes, None).attributes,
        }],
        category_id,
    )
    .await
}

/// Maps the store's "not found" bail onto a coded 404.
fn not_found(error: anyhow::Error) -> ApiError {
    crate::error_codes::store_not_found(&error, "subscription.not_found", "Subscription not found")
}

/// Asks an indexer what it can do, and thereby tests it (RD-080-11).
///
/// `t=caps` is the cheapest request that proves the address and the API key are both right,
/// so this is the test action as well as where the category list for the mapping UI comes
/// from. It pulls no results, so testing an indexer costs it almost nothing.
#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/{id}/caps",
    tag = "subscriptions",
    params(("id" = SubscriptionId, Path)),
    responses(
        (status = 200, body = rd_subscription::IndexerCaps),
        (status = 404, body = crate::error::ErrorBody),
        (status = 422, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody)
    )
)]
pub async fn subscription_caps(
    State(state): State<AppState>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<rd_subscription::IndexerCaps>, ApiError> {
    let subscription =
        state.database.subscription(id).await?.ok_or_else(|| {
            ApiError::not_found("subscription.not_found", "Subscription not found")
        })?;
    if subscription.kind != SubscriptionKind::Indexer {
        return Err(ApiError::unprocessable(
            "subscription.not_an_indexer",
            "Only an indexer subscription has capabilities",
        ));
    }
    let Some(reference) = subscription.secret_ref.as_deref() else {
        return Err(ApiError::unprocessable(
            "subscription.api_key_missing",
            "That indexer has no API key",
        ));
    };
    let api_key =
        state.secrets.get(reference).await.map_err(|_| {
            ApiError::unprocessable("subscription.api_key_missing", "Key unavailable")
        })?;
    fetch_caps(
        &state,
        &subscription.url,
        secrecy::ExposeSecret::expose_secret(&api_key),
    )
    .await
    .map(Json)
}

/// Body of `POST /api/v1/subscriptions/caps`.
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct CapsProbeRequest {
    /// The indexer's address, as it would be stored on the subscription.
    pub url: String,
    /// The key to ask with. Used for this one request and then dropped: nothing is stored, and
    /// a subscription only gains a key when it is saved.
    #[schema(write_only)]
    pub api_key: String,
}

/// Asks an indexer what it can do, before there is a subscription to ask for.
///
/// The `{id}` route above needs a stored subscription, because it resolves the key out of the
/// vault — so categories could only be mapped by saving the indexer first and reopening it,
/// which is not how anybody expects a form to behave. The request carries the address and the
/// key instead, and the chain underneath is the same one.
#[utoipa::path(
    post,
    path = "/api/v1/subscriptions/caps",
    tag = "subscriptions",
    request_body = CapsProbeRequest,
    responses(
        (status = 200, body = rd_subscription::IndexerCaps),
        (status = 422, body = crate::error::ErrorBody),
        (status = 502, body = crate::error::ErrorBody)
    )
)]
pub async fn probe_caps(
    State(state): State<AppState>,
    Json(request): Json<CapsProbeRequest>,
) -> Result<Json<rd_subscription::IndexerCaps>, ApiError> {
    let url: url::Url = request
        .url
        .trim()
        .parse()
        .map_err(|error: url::ParseError| {
            ApiError::unprocessable("subscription.url_invalid", error.to_string())
        })?;
    if request.api_key.trim().is_empty() {
        return Err(ApiError::unprocessable(
            "subscription.api_key_missing",
            "That indexer has no API key",
        ));
    }
    fetch_caps(&state, &url, request.api_key.trim())
        .await
        .map(Json)
}

/// The shared half: build the `t=caps` address, fetch it and parse the answer.
///
/// Every message about a failure carries the redacted address only — the real one has the key
/// in its query, and this is the one place both routes could leak it from.
async fn fetch_caps(
    state: &AppState,
    base: &url::Url,
    api_key: &str,
) -> Result<rd_subscription::IndexerCaps, ApiError> {
    let url = rd_subscription::build_caps_query(base, api_key)
        .map_err(|error| ApiError::unprocessable("subscription.url_invalid", error.to_string()))?;

    let network = state.scheduler.direct_client(&url).await.map_err(|error| {
        ApiError::bad_gateway("subscription.client_unavailable", error.to_string())
    })?;
    let body = rd_http::fetch_conditional(
        &network.client,
        url.clone(),
        &network.headers,
        rd_subscription::MAX_CAPS_BYTES,
    )
    .await
    .map_err(|error| {
        ApiError::bad_gateway(
            "subscription.caps_failed",
            format!("{error} ({})", rd_subscription::redact_query(&url)),
        )
    })?;
    let body = body.body.unwrap_or_default();
    rd_subscription::parse_caps(&body)
        .map_err(|error| ApiError::bad_gateway("subscription.caps_invalid", error.to_string()))
}
