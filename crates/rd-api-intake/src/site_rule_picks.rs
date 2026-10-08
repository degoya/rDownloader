//! The pick board over REST (RD-1170-03): a series page's entries, listed by a two-stage site
//! rule, chosen here, and resolved one after the other into LinkGrabber packages.
//!
//! **Two ways onto the board.** A pasted series page lands here by itself: the intake's crawl
//! pass lists it and answers `site_rules.pick_waiting` with the list's id. `POST
//! /api/v1/collector/picks` lists one address directly, which is what an agent does before it
//! chooses.
//!
//! **Resolving runs beside the request.** One entry is one captcha a person solves in the
//! broker, which takes as long as it takes; the request that chose the entries answers at once
//! with the queue, and the page reports its progress ("3 of 8") until the round ends or is
//! stopped. Each entry's links reach the LinkGrabber through the ordinary intake, as one
//! package named after the release, so the blocklist, the routing rules and the online check
//! treat them exactly as a pasted link.

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, State},
};
use rd_plugin_ext::{
    EntryState, PickDelivery, PickError, PickJob, PickPage, RuleOutcome, SiteRules,
};
use url::Url;

use crate::{
    AppState,
    dto::{CaptureLinkRequest, CollectorIntakeRequest, MessageResponse},
    error::ApiError,
    site_rule_picks_dto::{
        CollectorPickEntryResponse, CollectorPickResponse, CollectorPicksResponse,
        CreateCollectorPickRequest, ResolveCollectorPickRequest,
    },
};

#[utoipa::path(
    get,
    path = "/api/v1/collector/picks",
    tag = "collector",
    responses((status = 200, body = CollectorPicksResponse))
)]
pub async fn list_collector_picks(
    State(state): State<AppState>,
) -> Result<Json<CollectorPicksResponse>, ApiError> {
    let pages = match state.crawlers.rules() {
        Some(rules) => rules.picks().pages().iter().map(page_response).collect(),
        None => Vec::new(),
    };
    Ok(Json(CollectorPicksResponse { pages }))
}

#[utoipa::path(
    post,
    path = "/api/v1/collector/picks",
    tag = "collector",
    request_body = CreateCollectorPickRequest,
    responses((status = 200, body = CollectorPickResponse))
)]
pub async fn create_collector_pick(
    State(state): State<AppState>,
    Json(request): Json<CreateCollectorPickRequest>,
) -> Result<Json<CollectorPickResponse>, ApiError> {
    let address = Url::parse(request.address.trim())
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .ok_or_else(|| {
            ApiError::bad_request(
                "site_rules.invalid_address",
                "That is not an http or https address",
            )
            .with_param("address", request.address.clone())
        })?;
    let rules = rules(&state)?;
    match rules.consult(&address).await {
        Some(RuleOutcome::Listed { page, .. }) => rules
            .picks()
            .page(&page.id)
            .map(|page| Json(page_response(&page)))
            .ok_or_else(not_found),
        Some(RuleOutcome::Crawled { rule, .. }) => Err(ApiError::bad_request(
            "site_rules.no_pick",
            "The rule for this page resolves its links at once; collect it in the LinkGrabber",
        )
        .with_param("rule", rule)),
        Some(RuleOutcome::Refused { rule, error }) => Err(ApiError::bad_request(
            error.code(),
            format!("{rule}: {error}"),
        )
        .with_param("rule", rule)),
        None => Err(ApiError::bad_request(
            "site_rules.not_claimed",
            "No site rule that is switched on claims this address",
        )),
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/collector/picks/{id}",
    tag = "collector",
    params(("id" = String, Path, description = "The listed page, as the board names it")),
    responses((status = 200, body = CollectorPickResponse))
)]
pub async fn get_collector_pick(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CollectorPickResponse>, ApiError> {
    rules(&state)?
        .picks()
        .page(&id)
        .map(|page| Json(page_response(&page)))
        .ok_or_else(not_found)
}

#[utoipa::path(
    delete,
    path = "/api/v1/collector/picks/{id}",
    tag = "collector",
    params(("id" = String, Path, description = "The listed page, as the board names it")),
    responses((status = 200, body = MessageResponse))
)]
pub async fn delete_collector_pick(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MessageResponse>, ApiError> {
    if !rules(&state)?.picks().remove(&id) {
        return Err(not_found());
    }
    Ok(Json(MessageResponse::new(
        "site_rules.pick_discarded",
        "The list was discarded",
    )))
}

#[utoipa::path(
    post,
    path = "/api/v1/collector/picks/{id}/resolve",
    tag = "collector",
    request_body = ResolveCollectorPickRequest,
    params(("id" = String, Path, description = "The listed page, as the board names it")),
    responses((status = 200, body = CollectorPickResponse))
)]
pub async fn resolve_collector_pick(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<ResolveCollectorPickRequest>,
) -> Result<Json<CollectorPickResponse>, ApiError> {
    let rules = rules(&state)?;
    let (page, round) = rules
        .picks()
        .queue(&id, &request.entries)
        .map_err(refusal)?;
    if let Some(round) = round {
        let rules = Arc::clone(&rules);
        let shutdown = state.shutdown.clone();
        let delivery = CollectorDelivery {
            state: state.clone(),
        };
        tokio::spawn(async move {
            tokio::select! {
                () = rules.work_picks(&id, round, &delivery) => {}
                () = shutdown.cancelled() => {}
            }
        });
    }
    Ok(Json(page_response(&page)))
}

#[utoipa::path(
    post,
    path = "/api/v1/collector/picks/{id}/cancel",
    tag = "collector",
    params(("id" = String, Path, description = "The listed page, as the board names it")),
    responses((status = 200, body = CollectorPickResponse))
)]
pub async fn cancel_collector_pick(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CollectorPickResponse>, ApiError> {
    rules(&state)?
        .picks()
        .cancel(&id)
        .map(|page| Json(page_response(&page)))
        .ok_or_else(not_found)
}

/// Hands a resolved entry to the ordinary intake: one package, named after the release.
struct CollectorDelivery {
    state: AppState,
}

#[async_trait]
impl PickDelivery for CollectorDelivery {
    async fn deliver(&self, job: &PickJob, group: rd_siterules::CrawlGroup) -> Result<u32, String> {
        let request = CollectorIntakeRequest {
            text: None,
            // Somebody chose these entries by hand, in the LinkGrabber or through a tool.
            source: rd_core::IngressSource::Manual,
            source_label: Some(job.rule.name.clone()),
            package_name: group.name.or_else(|| job.label.clone()),
            password: None,
            links: group
                .links
                .into_iter()
                .map(|link| CaptureLinkRequest {
                    url: link.url,
                    file_name: None,
                    request: None,
                })
                .collect(),
        };
        crate::collector_handlers::collector_intake_inner(&self.state, request)
            .await
            .map(|response| u32::try_from(response.candidates.len()).unwrap_or(u32::MAX))
            .map_err(|error| error.code().to_owned())
    }
}

/// The rules in force, or the refusal an installation without any gives.
fn rules(state: &AppState) -> Result<Arc<SiteRules>, ApiError> {
    state.crawlers.rules().cloned().ok_or_else(not_found)
}

fn not_found() -> ApiError {
    ApiError::not_found(
        "site_rules.pick_not_found",
        "No list with that id; it was discarded, or the service restarted",
    )
}

fn refusal(error: PickError) -> ApiError {
    match error {
        PickError::NotFound => not_found(),
        PickError::NoEntry(index) => {
            ApiError::bad_request(error.code(), "The list has no entry with that index")
                .with_param("entry", index)
        }
    }
}

/// One page as the interface and the tools read it.
fn page_response(page: &PickPage) -> CollectorPickResponse {
    let entries = page
        .list
        .entries
        .iter()
        .zip(&page.progress)
        .enumerate()
        .map(|(index, (entry, progress))| CollectorPickEntryResponse {
            index,
            label: entry.label.clone(),
            attributes: entry.attributes.clone(),
            state: progress.state.as_str().to_owned(),
            code: progress.code.clone(),
            links: progress.links,
        })
        .collect();
    CollectorPickResponse {
        id: page.id.clone(),
        rule: page.rule.name.clone(),
        rule_id: page.rule.id.clone(),
        address: page.address.to_string(),
        package_name: page.package_name.clone(),
        created_at: page.created_at.to_rfc3339(),
        running: page.running,
        total: page.total,
        finished: page.finished,
        waiting_for_captcha: page
            .progress
            .iter()
            .any(|progress| progress.state == EntryState::Captcha),
        entries,
    }
}
