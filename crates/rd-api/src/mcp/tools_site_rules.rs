//! MCP tools for writing a site rule (RD-120-32).
//!
//! RD-120-29 covered reading the rules and switching them; writing one was out because a rule
//! is tried against a live page until its selectors hold. `test_site_rule` is that trying, on
//! the same route the editor calls, so a caller can do what the editor does in the same order.
//! Since RD-130-07 every rule is the person's own, the ones imported from the signed release
//! file included, so all of them can be written here.

use axum::{
    Json,
    extract::{Path, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
    params_config::IdParams,
    params_handling::{SiteRuleParams, TestSiteRuleParams, UpdateSiteRuleParams, body},
};
use crate::site_rules_handlers as rules;

#[tool_router(router = site_rules_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Create a site rule: a step program that finds the links on a release page. `rule` is the rule document: {id, name, group, version, match: {hosts, paths}, steps: [{kind: fetch}, {kind: regex, pattern, into, all}, ...], package, probe, checked} — list_site_rules shows the document of every rule under `rule`. Its id must be new. enabled defaults to false, as in the editor. Try it with test_site_rule first."
    )]
    pub async fn create_site_rule(
        &self,
        Parameters(params): Parameters<SiteRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "rule": params.rule,
                "enabled": params.enabled.unwrap_or(false),
            }))?;
            let Json(answer) =
                rules::create_site_rule(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Replace a site rule (id as list_site_rules gives it). `rule` is the whole rule document and its id must equal `id`; renaming a rule is a delete and a create."
    )]
    pub async fn update_site_rule(
        &self,
        Parameters(params): Parameters<UpdateSiteRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "rule": params.rule,
                "enabled": params.enabled.unwrap_or(false),
            }))?;
            let Json(answer) =
                rules::update_site_rule(State(self.state.clone()), Path(params.id), Json(request))
                    .await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete a site rule. To keep it but stop it being consulted, switch it off with set_site_rule_enabled instead."
    )]
    pub async fn delete_site_rule(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        respond(
            rules::delete_site_rule(State(self.state.clone()), Path(params.id))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "Try a site rule against a live page without storing it: fetches `address` and runs the rule's steps, answering with the links it found and where each step stopped."
    )]
    pub async fn test_site_rule(
        &self,
        Parameters(params): Parameters<TestSiteRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "rule": params.rule,
                "address": params.address,
            }))?;
            let Json(answer) =
                rules::test_site_rule(State(self.state.clone()), Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }
}
