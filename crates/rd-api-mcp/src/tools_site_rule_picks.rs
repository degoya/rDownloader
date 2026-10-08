//! MCP tools for choosing a series page's entries before they are resolved (RD-1170-03).
//!
//! The owner's wish of 2026-10-07: an agent writes the two-stage rule itself and drives the
//! choice -- lists a page's releases, picks some, starts resolving them. The captchas stay a
//! person's: each resolved entry asks one in the broker, and the tools say so rather than wait
//! for it, so the agent can tell the person what to solve.

use axum::{
    Json,
    extract::{Path, State},
};
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::Deserialize;

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
    params_config::IdParams,
    params_handling::body,
};
use crate::site_rule_picks as picks;

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct PageAddressParams {
    /// The page, an http or https address a two-stage site rule (one with `groups.pick`)
    /// claims -- a series page, say.
    pub address: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ResolveEntriesParams {
    /// The list's id, as list_page_entries or list_page_picks give it.
    pub id: String,
    /// The entries to resolve, by their `index`.
    pub entries: Vec<usize>,
}

#[tool_router(router = site_rule_picks_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List the entries of a page a two-stage site rule claims (a rule whose `groups` carries `pick`, such as serienjunkies.org's): runs the rule's own steps only, so nothing is resolved and no captcha is asked. Answers the list's `id` and its `entries`, each with `index`, `label` (the release name) and `attributes` -- season, episode, resolution, language, hoster, as the rule reads them; a season pack has no episode. Choose by those and call resolve_page_entries. A page pasted with collect_links lands on the same board by itself, answering the code site_rules.pick_waiting with the list's id."
    )]
    pub async fn list_page_entries(
        &self,
        Parameters(params): Parameters<PageAddressParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({ "address": params.address }))?;
            let Json(answer) =
                picks::create_collector_pick(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Every listed page waiting for a choice, oldest first, with its entries and how far resolving got: `running`, `finished` of `total` (\"3 of 8\"), and per entry its `state`."
    )]
    pub async fn list_page_picks(&self) -> McpToolResult {
        respond(
            picks::list_collector_picks(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "One listed page with its entries and progress (id from list_page_entries or list_page_picks); poll this while entries resolve."
    )]
    pub async fn get_page_pick(&self, Parameters(params): Parameters<IdParams>) -> McpToolResult {
        respond(
            picks::get_collector_pick(State(self.state.clone()), Path(params.id))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Resolve the chosen entries of a listed page, one after the other; answers at once with the queue. Each entry of a rule with a captcha step asks one captcha, which a person solves in the captcha broker (web interface or browser extension) -- no tool solves it. While an entry waits, its state is `captcha` and `waiting_for_captcha` is true: tell the user a captcha is waiting, and poll get_page_pick. A resolved entry's links land in the LinkGrabber as one package named after the release (`done`, with `links`); an unanswered captcha puts the entry back to `pending` with code site_rules.captcha_failed, and it can be resolved again; `failed` carries the page's refusal. Entries already done or underway are left alone."
    )]
    pub async fn resolve_page_entries(
        &self,
        Parameters(params): Parameters<ResolveEntriesParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({ "entries": params.entries }))?;
            let Json(answer) = picks::resolve_collector_pick(
                State(self.state.clone()),
                Path(params.id),
                Json(request),
            )
            .await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Stop resolving a listed page: queued entries and the one waiting for its captcha go back to `pending` (code site_rules.pick_cancelled); what is done stays in the LinkGrabber."
    )]
    pub async fn cancel_page_pick(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        respond(
            picks::cancel_collector_pick(State(self.state.clone()), Path(params.id))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Discard a listed page and stop whatever it is resolving. The LinkGrabber keeps what was resolved."
    )]
    pub async fn discard_page_pick(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        respond(
            picks::delete_collector_pick(State(self.state.clone()), Path(params.id))
                .await
                .map(|Json(answer)| answer),
        )
    }
}
