//! MCP tools for pausing the whole queue for a while and switching a bandwidth profile on by
//! hand (RD-190-20).
//!
//! Both end on their own — the pause at its time, the switch at the schedule's next change or
//! at a time — so an agent that sets one and forgets about it leaves nothing behind for good.
//! Editing profiles and the weekly schedule stays out (`mcp_coverage`); these tools only pick
//! one of the profiles a person made, listed by list_bandwidth_profiles.

use axum::{Json, extract::State};
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::Deserialize;

use super::{
    RdMcpServer,
    error::{McpToolResult, json_result, respond},
    params_handling::body,
};
use crate::{bandwidth_handlers, bandwidth_manual_handlers, queue_pause_handlers};

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct PauseQueueParams {
    /// How long to pause, in minutes from now (1 to 43200); give this or `until`.
    #[serde(default)]
    pub minutes: Option<u32>,
    /// When the pause ends, as an RFC 3339 time (e.g. `2026-10-02T18:00:00Z`); give this or
    /// `minutes`.
    #[serde(default)]
    pub until: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SwitchProfileParams {
    /// The profile to switch to (id from list_bandwidth_profiles); absent for no limits at all.
    #[serde(default)]
    pub profile_id: Option<String>,
    /// When the schedule takes over again: `next_switch` (its next change), `at` (the time in
    /// `until`) or `never` (only return_to_bandwidth_schedule ends it).
    pub ends: String,
    /// The end for `ends: at`, as an RFC 3339 time, at most 30 days ahead.
    #[serde(default)]
    pub until: Option<String>,
}

#[tool_router(router = pause_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read whether the whole queue is paused for a while: until when, and how many files the pause stopped. Not paused answers `paused: false`."
    )]
    pub async fn get_queue_pause(&self) -> McpToolResult {
        let Json(answer) = queue_pause_handlers::get_queue_pause(State(self.state.clone())).await;
        json_result(&answer)
    }

    #[tool(
        description = "Pause the whole queue for a while: every waiting and running download is paused, nothing new starts, and at the end the downloads this pause stopped resume by themselves (one paused before stays paused). Give `minutes` or `until`, within 30 days. Pausing again while paused moves the end. Answers queue.pause_end_invalid for a missing, past or too distant end."
    )]
    pub async fn pause_queue(
        &self,
        Parameters(params): Parameters<PauseQueueParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "minutes": params.minutes,
                "until": params.until,
            }))?;
            let Json(answer) =
                queue_pause_handlers::pause_queue(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "End a timed queue pause now: the downloads it stopped resume and new ones may start. Answers how many resumed; 0 when no pause was in force."
    )]
    pub async fn resume_queue(&self) -> McpToolResult {
        respond(
            queue_pause_handlers::resume_queue(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Read the bandwidth state: the active profile and whether the schedule or a switch by hand chose it (`source`, and `manual` with its end), the next change, the limits in force and the traffic budgets used today and this month."
    )]
    pub async fn get_bandwidth_status(&self) -> McpToolResult {
        respond(
            bandwidth_handlers::bandwidth_status(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "List the bandwidth profiles with their ids, limits and budgets, for switch_bandwidth_profile. Profiles and the weekly schedule are edited in the interface."
    )]
    pub async fn list_bandwidth_profiles(&self) -> McpToolResult {
        respond(
            bandwidth_handlers::list_profiles(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Switch a bandwidth profile on by hand, in front of the weekly schedule, until `ends`: `next_switch` (the schedule's next change), `at` (the time in `until`, within 30 days) or `never` (until return_to_bandwidth_schedule). Leave `profile_id` out for no limits at all. Answers the bandwidth status; bandwidth.profile_not_found or bandwidth.manual_end_invalid when refused."
    )]
    pub async fn switch_bandwidth_profile(
        &self,
        Parameters(params): Parameters<SwitchProfileParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "profile_id": params.profile_id,
                "ends": params.ends,
                "until": params.until,
            }))?;
            let Json(answer) = bandwidth_manual_handlers::switch_bandwidth_profile(
                State(self.state.clone()),
                Json(request),
            )
            .await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "End a bandwidth profile switched on by hand, so the weekly schedule decides again. Answers the bandwidth status."
    )]
    pub async fn return_to_bandwidth_schedule(&self) -> McpToolResult {
        respond(
            bandwidth_manual_handlers::return_to_bandwidth_schedule(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }
}
