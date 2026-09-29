//! MCP tools for recording livestreams on a timetable, and for recording one now (RD-120-55).
//!
//! RD-120-29 left these out as "a calendar grid", "a history panel" and "whatever happens to
//! be live at that second". None of the three is one of the owner's four marks. A schedule is a
//! row the route validates against the planner's own rules -- an unknown zone, no weekdays or a
//! window of zero is refused with the route's code -- so a caller that cannot see the grid gets
//! the same answer the form does. Recording now reads a public stream the way a download
//! does; the package it starts is this machine's and can be cancelled like any other.
//!
//! The schedule body is a tree (a weekly or a one-off kind, flattened into the body), so the
//! write tools take it as `definition`, as the channel tools beside them do.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, from_definition, parse_id, respond},
    params_config::IdParams,
    params_delivery::{DefinitionParams, UpdateDefinitionParams},
    params_handling::{body, public},
    params_remaining::{RecordNowParams, StreamRunsParams, object},
};
use crate::stream_schedule_handlers as schedules;

#[tool_router(router = stream_schedules_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List the livestream recording schedules: which channel, weekly or one-off, the time zone, the window and its lead and trail minutes, and whether each is switched on."
    )]
    pub async fn list_stream_schedules(&self) -> McpToolResult {
        respond(
            schedules::list_stream_schedules(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Create a recording schedule. `definition` is the REST body of POST /api/v1/streams/schedules: channel_id (from list_stream_channels), name, enabled, kind: weekly (days as ISO weekdays 1-7, start_minute after local midnight) or once (start, an RFC 3339 moment), timezone (an IANA zone such as Europe/Berlin; an offset is refused), window_minutes, lead_minutes, trail_minutes, replay_from_start. A schedule that could never fire is refused with the planner's code."
    )]
    pub async fn create_stream_schedule(
        &self,
        Parameters(params): Parameters<DefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let request = from_definition(params.definition)?;
            let (_, Json(created)) =
                schedules::create_stream_schedule(State(self.state.clone()), Json(request)).await?;
            Ok(created)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Replace one recording schedule (id from list_stream_schedules). `definition` is the same body as create; it is a replacement, so send every field you want to keep. Occurrences not yet started are planned again."
    )]
    pub async fn update_stream_schedule(
        &self,
        Parameters(params): Parameters<UpdateDefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = from_definition(params.definition)?;
            let Json(updated) = schedules::update_stream_schedule(
                State(self.state.clone()),
                Path(id),
                Json(request),
            )
            .await?;
            Ok(updated)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete one recording schedule (id from list_stream_schedules). Destructive; recordings it already made are kept."
    )]
    pub async fn delete_stream_schedule(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            schedules::delete_stream_schedule(State(self.state.clone()), Path(id)).await?;
            Ok(serde_json::json!({ "deleted": params.id }))
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the planned, running, finished and missed occurrences of the recording schedules, newest first; `schedule_id` narrows it to one. A missed occurrence is a row with its error, not an absence."
    )]
    pub async fn list_stream_runs(
        &self,
        Parameters(params): Parameters<StreamRunsParams>,
    ) -> McpToolResult {
        let result = async {
            let query = body(object(&[
                ("schedule_id", params.schedule_id.into()),
                ("limit", params.limit.into()),
            ]))?;
            let Json(runs) =
                schedules::list_stream_runs(State(self.state.clone()), Query(query)).await?;
            Ok(runs)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Start recording a livestream now: `url` is the stream's address, with an optional package name, quality (best, 720p, ...) and category_id. Answers with the package the recording runs in; it is controlled and cancelled like any other download. A saved channel with the same address lends it its recording policy."
    )]
    pub async fn record_stream_now(
        &self,
        Parameters(params): Parameters<RecordNowParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(object(&[
                ("url", params.url.into()),
                ("name", params.name.into()),
                ("quality", params.quality.into()),
                ("category_id", params.category_id.into()),
            ]))?;
            let (_, Json(package)) =
                crate::stream_handlers::record_now(State(self.state.clone()), Json(request))
                    .await?;
            public(&package)
        }
        .await;
        respond(result)
    }
}
