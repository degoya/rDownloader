//! MCP tools for what the automation, notification and routing editors show beside the rows
//! they edit: histories, catalogues and previews (RD-120-55).
//!
//! RD-120-29 left each out as "the editor's autocomplete source", "a history panel" or "a live
//! preview in the rule editor". Those are reasons the interface draws them, not reasons an
//! agent should not have them -- an agent writing an automation needs the vocabulary more than
//! a person with a dropdown does, and one asking why a notification never arrived needs the
//! delivery history. None of them hands out a secret, takes one in, gives a consent or
//! changes anything outside this machine: the dry run evaluates and acts on nothing, and the
//! regex tester compiles a pattern against samples the caller sends.

use axum::{
    Json,
    extract::{Path, Query, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, json_result, parse_id, respond},
    params_config::IdParams,
    params_handling::body,
    params_remaining::{
        AutomationRunsParams, DeliveriesParams, DryRunParams, TestRegexParams, object,
    },
};
use crate::automation_handlers as automations;

#[tool_router(router = editors_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read what an automation may be built from: the triggers, the condition fields and operators, the action kinds, and the limits (actions per automation, condition depth). The words create_automation and update_automation accept."
    )]
    pub async fn get_automation_vocabulary(&self) -> McpToolResult {
        let Json(vocabulary) = automations::automation_vocabulary().await;
        json_result(&vocabulary)
    }

    #[tool(
        description = "List the newest automation runs: which automation and version, the event and package it ran for, its state, the action it reached, attempts and message. `automation_id` (from list_automations) narrows it to one; `limit` is 1-500."
    )]
    pub async fn list_automation_runs(
        &self,
        Parameters(params): Parameters<AutomationRunsParams>,
    ) -> McpToolResult {
        let result = async {
            let query = body(object(&[
                ("automation_id", params.automation_id.into()),
                ("limit", params.limit.into()),
            ]))?;
            let Json(runs) =
                automations::list_automation_runs(State(self.state.clone()), Query(query)).await?;
            Ok(runs)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the saved versions of one automation (id from list_automations): version number, trigger, condition, actions and when each was saved."
    )]
    pub async fn list_automation_versions(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(versions) =
                automations::list_automation_versions(State(self.state.clone()), Path(id)).await?;
            Ok(versions)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Ask, per active automation, whether a trigger would match it and whether its conditions would hold, without running anything. `trigger` is a word from get_automation_vocabulary; `package_id` (from list_packages) is the package the conditions are judged against."
    )]
    pub async fn dry_run_automations(
        &self,
        Parameters(params): Parameters<DryRunParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(object(&[
                ("trigger", params.trigger.into()),
                ("package_id", params.package_id.into()),
            ]))?;
            let Json(matches) =
                automations::dry_run_automations(State(self.state.clone()), Json(request)).await?;
            Ok(matches)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the newest notification deliveries: which rule and target, the event, the title and text sent, the state (pending, delivered, failed, ...), the attempt count and the target's status code with a redacted excerpt of its answer. `limit` is 1-500."
    )]
    pub async fn list_notification_deliveries(
        &self,
        Parameters(params): Parameters<DeliveriesParams>,
    ) -> McpToolResult {
        let result = async {
            let query = body(object(&[("limit", params.limit.into())]))?;
            let Json(deliveries) =
                crate::notify_handlers::list_deliveries(State(self.state.clone()), Query(query))
                    .await?;
            Ok(deliveries)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the destinations installed notification plugins provide: plugin_id (what a plugin target carries in config.plugin_id), name and version."
    )]
    pub async fn list_notification_destinations(&self) -> McpToolResult {
        let Json(destinations) =
            crate::notify_handlers::list_destinations(State(self.state.clone())).await;
        json_result(&destinations)
    }

    #[tool(
        description = "Try a routing rule's regular expression before writing it: whether it compiles (and the error if not), and for each of up to 50 sample texts whether and where it matches."
    )]
    pub async fn test_category_regex(
        &self,
        Parameters(params): Parameters<TestRegexParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "pattern": params.pattern,
                "samples": params.samples,
            }))?;
            let Json(answer) = crate::regex_tester::test_category_rule_regex(Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }
}
