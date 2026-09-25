//! MCP tools for a torrent's panel: detail, file plan, trackers and seeding (RD-120-32).
//!
//! RD-120-29 called the panel "a live technical panel measured in seconds". It is also the only
//! place a torrent's files, trackers and seeding are decided, so a caller that cannot open it
//! had no way to do any of that. The six read-outs are one tool with a `view`, all priced
//! `api:read` like their routes; the writes are one tool per decision, priced `api:queue`.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, api_error, parse_id, respond},
    params_config::IdParams,
    params_handling::{
        EngineView, EngineViewParams, IdBodyParams, SeedingParams, TorrentView, TorrentViewParams,
        TrackerAction, TrackersParams, body,
    },
};
use crate::{ApiError, torrent_control as control, torrent_trackers as trackers};

/// Serialises one route's answer, so the arms of a `view` match share a type.
fn value<T: serde::Serialize>(answer: T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(answer)
        .map_err(|error| ApiError::bad_request("request.body_invalid", error.to_string()))
}

#[tool_router(router = torrent_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read a queued torrent (the download id from list_downloads). view=summary: files, sizes, progress and the file plan; peers: connected peers, paged with limit and cursor; pieces: piece availability; stats: aggregate transfer figures; trackers: trackers and their announce state; seeding: the seeding policy in force and where each value comes from."
    )]
    pub async fn get_torrent_details(
        &self,
        Parameters(params): Parameters<TorrentViewParams>,
    ) -> McpToolResult {
        let id: rd_core::DownloadId = match parse_id(&params.id) {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let state = State(self.state.clone());
        let result = async {
            match params.view {
                TorrentView::Summary => {
                    value(control::get_download_torrent(state, Path(id)).await?.0)
                }
                TorrentView::Peers => value(
                    trackers::torrent_peers(
                        state,
                        Path(id),
                        Query(trackers::PeerPageQuery {
                            limit: params.limit,
                            cursor: params.cursor,
                        }),
                    )
                    .await?
                    .0,
                ),
                TorrentView::Pieces => value(trackers::torrent_pieces(state, Path(id)).await?.0),
                TorrentView::Stats => value(trackers::torrent_stats(state, Path(id)).await?.0),
                TorrentView::Trackers => value(trackers::list_trackers(state, Path(id)).await?.0),
                TorrentView::Seeding => {
                    value(control::get_download_seeding(state, Path(id)).await?.0)
                }
            }
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Decide which files of a queued torrent are fetched and in which order. `body` is the REST body of PUT /api/v1/downloads/{id}/torrent/plan: included and excluded file indices, priorities [{index, priority}], exclusion_patterns (globs), sequential. Read the files with get_torrent_details view=summary first."
    )]
    pub async fn set_torrent_file_plan(
        &self,
        Parameters(params): Parameters<IdBodyParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(serde_json::Value::Object(params.body))?;
            let Json(answer) = control::put_download_torrent_plan(
                State(self.state.clone()),
                Path(id),
                Json(request),
            )
            .await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Act on a queued torrent's trackers. action=set replaces the list with `trackers`, each {id} to keep an existing one (ids from get_torrent_details view=trackers) or {url} to add one, with an optional tier; the list is authoritative, so an entry left out is removed. reannounce announces now; scrape asks for seeder and leecher counts."
    )]
    pub async fn update_torrent_trackers(
        &self,
        Parameters(params): Parameters<TrackersParams>,
    ) -> McpToolResult {
        let id: rd_core::DownloadId = match parse_id(&params.id) {
            Ok(id) => id,
            Err(error) => return Ok(api_error(error)),
        };
        let state = State(self.state.clone());
        let result = async {
            match params.action {
                TrackerAction::Set => {
                    let request = body(serde_json::json!({
                        "trackers": params.trackers.unwrap_or_default(),
                    }))?;
                    value(
                        trackers::put_trackers(state, Path(id), Json(request))
                            .await?
                            .0,
                    )
                }
                TrackerAction::Reannounce => value(trackers::reannounce(state, Path(id)).await?.0),
                TrackerAction::Scrape => value(trackers::scrape_trackers(state, Path(id)).await?.0),
            }
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Override how long one queued torrent seeds. `body` is the REST body of PUT /api/v1/downloads/{id}/torrent/seeding: enabled, ratio, time_minutes, time_unlimited; a field left out keeps inheriting. clear=true drops the override so the category and global values apply again."
    )]
    pub async fn set_torrent_seeding(
        &self,
        Parameters(params): Parameters<SeedingParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let state = State(self.state.clone());
            let Json(answer) = if params.clear.unwrap_or(false) {
                control::delete_download_seeding(state, Path(id)).await?
            } else {
                let request = body(serde_json::Value::Object(params.body))?;
                control::put_download_seeding(state, Path(id), Json(request)).await?
            };
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Stop a finished torrent seeding now, whatever its ratio and time targets say."
    )]
    pub async fn stop_seeding(&self, Parameters(params): Parameters<IdParams>) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(answer) =
                crate::torrent_handlers::stop_seeding(State(self.state.clone()), Path(id)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Set the seeding policy every torrent of one category inherits (category id from list_configuration section categories). `body` is {enabled, ratio, time_minutes, time_unlimited}; clear=true drops the category's policy so the global one applies."
    )]
    pub async fn set_category_seeding(
        &self,
        Parameters(params): Parameters<SeedingParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let state = State(self.state.clone());
            let Json(answer) = if params.clear.unwrap_or(false) {
                control::delete_category_seeding(state, Path(id)).await?
            } else {
                let request = body(serde_json::Value::Object(params.body))?;
                control::put_category_seeding(state, Path(id), Json(request)).await?
            };
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Read the torrent engine. view=capabilities: what it supports (sequential download, encryption, DHT, ...); view=network_status: bound interface, kill switch, proxy, blocklist and UPnP state."
    )]
    pub async fn get_torrent_engine(
        &self,
        Parameters(params): Parameters<EngineViewParams>,
    ) -> McpToolResult {
        let state = State(self.state.clone());
        match params.view {
            EngineView::Capabilities => respond(Ok::<_, ApiError>(
                crate::torrent_handlers::torrent_capabilities(state).await.0,
            )),
            EngineView::NetworkStatus => respond(Ok::<_, ApiError>(
                crate::torrent_handlers::torrent_network_status(state)
                    .await
                    .0,
            )),
        }
    }

    #[tool(
        description = "List this machine's network interfaces, the names the torrent engine can be bound to in the settings document (torrent bind interface)."
    )]
    pub async fn list_network_interfaces(&self) -> McpToolResult {
        respond(Ok::<_, ApiError>(
            crate::torrent_handlers::torrent_interfaces().await.0,
        ))
    }
}
