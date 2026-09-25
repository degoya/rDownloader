//! MCP tools for the LinkGrabber's candidate level: one link at a time (RD-120-32).
//!
//! RD-120-29 left this out as "direct manipulation of a work-in-progress list". The owner's
//! answer was that a list a caller cannot see is an argument for a listing tool, not against the
//! tools that act on its rows — so [`list_candidates`](RdMcpServer::list_candidates) comes
//! first, and every other tool here takes an id it hands out. Each one calls its route's handler
//! with the extractors built by hand; none of them decides anything the route does not.

use axum::{
    Json,
    extract::{Path, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};
use serde::Serialize;

use super::{
    RdMcpServer,
    error::{McpToolResult, api_error, json_result, parse_id, parse_ids, respond},
    params::paginate,
    params_config::IdParams,
    params_handling::{
        CandidatePlanParams, CandidateView, CandidateViewParams, IdsParams, ListCandidatesParams,
        MediaPreviewKind, MediaPreviewParams, MirrorAction, MirrorParams, MirrorPreferenceParams,
        MoveCandidatesParams, ReorderMembersParams, UpdateCandidateParams, body, public,
    },
};
use crate::{ApiError, collector_handlers as collector, handlers};

/// One LinkGrabber link, with what the candidate tools need to act on it.
///
/// A projection rather than the row: the stored candidate also carries the captured request of
/// a browser hand-off and a consent record, and neither is anything a tool should repeat.
#[derive(Serialize)]
pub(crate) struct CandidateRow {
    pub id: String,
    pub batch_id: String,
    pub package_id: Option<String>,
    pub position: i64,
    pub url: String,
    pub state: rd_core::LinkCandidateState,
    pub file_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub provider: Option<String>,
    pub error: Option<String>,
    pub error_code: Option<String>,
    /// When a provider last said it holds the file in its cache (RD-120-36): a measurement
    /// with that time, not a promise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The slug of the provider that gave that cache answer (RD-130-11); only with `cached_at`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_by: Option<String>,
    /// `media`, `torrent` or `listing` when get_candidate_details has a view for this link.
    pub details: Vec<&'static str>,
    pub mirror: Option<rd_core::CandidateMirror>,
}

impl From<rd_core::LinkCandidate> for CandidateRow {
    fn from(candidate: rd_core::LinkCandidate) -> Self {
        let mut details = Vec::new();
        if candidate.media.is_some() {
            details.push("media");
        }
        if candidate.torrent.is_some() {
            details.push("torrent");
        }
        if candidate.listing.is_some() {
            details.push("listing");
        }
        Self {
            id: candidate.id.to_string(),
            batch_id: candidate.batch_id.to_string(),
            package_id: candidate.package_id.map(|id| id.to_string()),
            position: candidate.position,
            url: candidate.url.to_string(),
            state: candidate.state,
            file_name: candidate.file_name,
            size_bytes: candidate.size.map(rd_core::ByteCount::get),
            provider: candidate.provider,
            error: candidate.error,
            error_code: candidate.error_code,
            cached_at: candidate.cached_at,
            cached_by: candidate.cached_by,
            details,
            mirror: candidate.mirror,
        }
    }
}

#[derive(Serialize)]
struct PerIdResult {
    done: Vec<String>,
    errors: Vec<String>,
}

#[tool_router(router = candidates_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List the links in the LinkGrabber, one row per link, with the ids every other candidate tool takes. Filter by package_id or batch_id; paginated. `details` names which views get_candidate_details has for a link; `mirror` shows its mirror group."
    )]
    pub async fn list_candidates(
        &self,
        Parameters(params): Parameters<ListCandidatesParams>,
    ) -> McpToolResult {
        let result = async {
            let package_id = params
                .package_id
                .as_deref()
                .map(parse_id::<rd_core::CollectorPackageId>)
                .transpose()?;
            let batch_id = params
                .batch_id
                .as_deref()
                .map(parse_id::<rd_core::BatchId>)
                .transpose()?;
            let Json(mut rows) = handlers::list_candidates(State(self.state.clone())).await?;
            rows.retain(|candidate| {
                package_id.is_none_or(|id| candidate.package_id == Some(id))
                    && batch_id.is_none_or(|id| candidate.batch_id == id)
            });
            rows.sort_by_key(|candidate| (candidate.package_id, candidate.position));
            Ok(paginate(
                rows,
                params.limit,
                params.offset,
                CandidateRow::from,
            ))
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Empty the whole LinkGrabber: every link and package in it. Links already enqueued are not affected. Answers with how many links were removed."
    )]
    pub async fn clear_linkgrabber(&self) -> McpToolResult {
        respond(
            handlers::delete_candidates(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Remove links from the LinkGrabber by id (from list_candidates). A link being enqueued right now is refused with collector.candidate_busy."
    )]
    pub async fn delete_candidates(
        &self,
        Parameters(params): Parameters<IdsParams>,
    ) -> McpToolResult {
        let ids: Vec<rd_core::CandidateId> = match parse_ids(&params.ids) {
            Ok(ids) => ids,
            Err(error) => return Ok(api_error(error)),
        };
        if ids.is_empty() || ids.len() > 500 {
            return Ok(api_error(crate::error_codes::bulk_range(500)));
        }
        let mut outcome = PerIdResult {
            done: Vec::new(),
            errors: Vec::new(),
        };
        for id in ids {
            match handlers::delete_candidate(State(self.state.clone()), Path(id)).await {
                Ok(_) => outcome.done.push(id.to_string()),
                Err(error) => {
                    outcome
                        .errors
                        .push(format!("{id}: {} ({})", error.message(), error.code()))
                }
            }
        }
        json_result(&outcome)
    }

    #[tool(
        description = "Move links into another LinkGrabber package (package_id from list_collector) or into a new one named new_package_name. Answers with the target package."
    )]
    pub async fn move_candidates(
        &self,
        Parameters(params): Parameters<MoveCandidatesParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "ids": params.ids,
                "package_id": params.package_id,
                "new_package_name": params.new_package_name,
            }))?;
            let Json(package) =
                collector::move_candidates(State(self.state.clone()), Json(request)).await?;
            public(&package)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Set the order of the links inside one LinkGrabber package. `ids` must be exactly that package's links (list_candidates with package_id), each once, in the new order."
    )]
    pub async fn reorder_candidates(
        &self,
        Parameters(params): Parameters<ReorderMembersParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "package_id": params.package_id,
                "ids": params.ids,
            }))?;
            let Json(answer) =
                collector::reorder_candidates(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change one LinkGrabber link: the file name it downloads under, and for a media link the variant to fetch. Answers with the link."
    )]
    pub async fn update_candidate(
        &self,
        Parameters(params): Parameters<UpdateCandidateParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(serde_json::json!({
                "file_name": params.file_name,
                "media_variant": params.media_variant,
            }))?;
            let Json(candidate) =
                collector::rename_candidate(State(self.state.clone()), Path(id), Json(request))
                    .await?;
            Ok(CandidateRow::from(candidate))
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Enqueue a single LinkGrabber link on its own: it becomes its own download package and starts. Answers with the package."
    )]
    pub async fn enqueue_candidate(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        respond(
            handlers::enqueue_candidate(State(self.state.clone()), Path(params.id))
                .await
                .and_then(|(_, Json(package))| public(&package)),
        )
    }

    #[tool(
        description = "Read what one link offers before it is enqueued. view=media: the formats and presets of a video or audio link; view=torrent: the files of a torrent or magnet and the plan; view=listing: the files of a directory listing and which are excluded. list_candidates' `details` says which views a link has."
    )]
    pub async fn get_candidate_details(
        &self,
        Parameters(params): Parameters<CandidateViewParams>,
    ) -> McpToolResult {
        let id: rd_core::CandidateId = match parse_id(&params.id) {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let state = State(self.state.clone());
        match params.view {
            CandidateView::Media => respond(
                crate::media_handlers::get_candidate_media_formats(state, Path(id))
                    .await
                    .map(|Json(answer)| answer),
            ),
            CandidateView::Torrent => respond(
                crate::torrent_control::get_candidate_torrent(state, Path(id))
                    .await
                    .map(|Json(answer)| answer),
            ),
            CandidateView::Listing => respond(
                crate::remote_listing_handlers::get_candidate_listing(state, Path(id))
                    .await
                    .map(|Json(answer)| answer),
            ),
        }
    }

    #[tool(
        description = "Decide what one link will fetch. kind=media: `body` is {preset} or {criteria} (PUT /api/v1/collector/candidates/{id}/media/selection); kind=torrent: {included, excluded, priorities, exclusion_patterns, sequential} (PUT …/torrent/plan); kind=listing: {excluded} paths (PUT …/listing/plan). Read the current state with get_candidate_details first."
    )]
    pub async fn set_candidate_plan(
        &self,
        Parameters(params): Parameters<CandidatePlanParams>,
    ) -> McpToolResult {
        let id: rd_core::CandidateId = match parse_id(&params.id) {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let state = State(self.state.clone());
        let raw = serde_json::Value::Object(params.body);
        match params.kind {
            CandidateView::Media => respond(
                async {
                    let request = body(raw)?;
                    let Json(candidate) = crate::media_handlers::put_candidate_media_selection(
                        state,
                        Path(id),
                        Json(request),
                    )
                    .await?;
                    Ok::<_, ApiError>(CandidateRow::from(candidate))
                }
                .await,
            ),
            CandidateView::Torrent => respond(
                async {
                    let request = body(raw)?;
                    let Json(answer) = crate::torrent_control::put_candidate_torrent_plan(
                        state,
                        Path(id),
                        Json(request),
                    )
                    .await?;
                    Ok::<_, ApiError>(answer)
                }
                .await,
            ),
            CandidateView::Listing => respond(
                async {
                    let request = body(raw)?;
                    let Json(answer) = crate::remote_listing_handlers::put_candidate_listing_plan(
                        state,
                        Path(id),
                        Json(request),
                    )
                    .await?;
                    Ok::<_, ApiError>(answer)
                }
                .await,
            ),
        }
    }

    #[tool(
        description = "Try a media choice without storing it. kind=selection with {preset} or {criteria}: which format it would pick; kind=output with {template}: the file name it would produce."
    )]
    pub async fn preview_candidate_media(
        &self,
        Parameters(params): Parameters<MediaPreviewParams>,
    ) -> McpToolResult {
        let id: rd_core::CandidateId = match parse_id(&params.id) {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let state = State(self.state.clone());
        let raw = serde_json::Value::Object(params.body);
        match params.kind {
            MediaPreviewKind::Selection => respond(
                async {
                    let Json(answer) = crate::media_handlers::preview_media_selection(
                        state,
                        Path(id),
                        Json(body(raw)?),
                    )
                    .await?;
                    Ok::<_, ApiError>(answer)
                }
                .await,
            ),
            MediaPreviewKind::Output => respond(
                async {
                    let Json(answer) = crate::media_handlers::preview_media_output(
                        state,
                        Path(id),
                        Json(body(raw)?),
                    )
                    .await?;
                    Ok::<_, ApiError>(answer)
                }
                .await,
            ),
        }
    }

    #[tool(
        description = "Fetch a magnet link's metadata now, so its files can be read with get_candidate_details view=torrent and planned with set_candidate_plan."
    )]
    pub async fn resolve_candidate_torrent(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(answer) = crate::torrent_control::resolve_candidate_torrent(
                State(self.state.clone()),
                Path(id),
            )
            .await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Act on a link's mirror group (list_candidates shows `mirror`). action=pin: make this link the group's chosen mirror, above the standing preference; release: drop that pin; dissolve: take a proposed group apart so its links stand alone (a declared or size-confirmed group is refused)."
    )]
    pub async fn set_candidate_mirror(
        &self,
        Parameters(params): Parameters<MirrorParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let state = State(self.state.clone());
            let Json(answer) = match params.action {
                MirrorAction::Pin => collector::pin_mirror(state, Path(id)).await?,
                MirrorAction::Release => collector::release_mirror(state, Path(id)).await?,
                MirrorAction::Dissolve => collector::dissolve_mirror(state, Path(id)).await?,
            };
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Read the standing mirror preference: the quality, language and hoster the LinkGrabber prefers when one file is offered by several mirrors, and the hosters it hides."
    )]
    pub async fn get_mirror_preference(&self) -> McpToolResult {
        respond(
            collector::get_mirror_preference(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Set the standing mirror preference and re-choose every mirror group under it. Each of quality, language and hoster is optional; an absent or empty one is cleared. hidden_hosters lists the hosters the LinkGrabber hides; absent keeps the stored list, an empty list shows every hoster again."
    )]
    pub async fn set_mirror_preference(
        &self,
        Parameters(params): Parameters<MirrorPreferenceParams>,
    ) -> McpToolResult {
        let result = async {
            let hidden_hosters = match params.hidden_hosters {
                Some(hosters) => hosters,
                None => {
                    self.state
                        .database
                        .mirror_preference()
                        .await?
                        .hidden_hosters
                }
            };
            let request = body(serde_json::json!({
                "quality": params.quality,
                "language": params.language,
                "hoster": params.hoster,
                "hidden_hosters": hidden_hosters,
            }))?;
            let Json(answer) =
                collector::put_mirror_preference(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }
}
