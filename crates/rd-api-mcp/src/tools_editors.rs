//! MCP tools for what the automation, notification and routing editors show beside the rows
//! they edit: histories, catalogues and previews (RD-120-55).
//!
//! RD-120-29 left each out as "the editor's autocomplete source", "a history panel" or "a live
//! preview in the rule editor". Those are reasons the interface draws them, not reasons an
//! agent should not have them -- an agent writing an automation needs the vocabulary more than
//! a person with a dropdown does, and one asking why a notification never arrived needs the
//! delivery history. None of them hands out a secret, takes one in, gives a consent or
//! changes anything outside this machine: the dry run evaluates and acts on nothing, and the
//! regex tester compiles a pattern against samples the caller sends. The one writer here empties
//! an automation's version history (RD-1280-01), and asks the person first as every tool that
//! empties a store does.

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
        AutomationRunsParams, DeliveriesParams, DryRunParams, ResetAutomationVersionsParams,
        TestRegexParams, object,
    },
};
use crate::automation_handlers as automations;

#[tool_router(router = editors_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Read what an automation may be built from: the triggers, the condition fields and operators, the action kinds, and the limits (actions per automation, condition depth). The words create_automation and update_automation accept. The field `change` is the storage threshold's own: `blocked` when a storage root ran out of space, `released` when it has room again; `storage_target` is its too: the key of the storage target that blocked or was released, compared with `equals` -- the `target` get_storage_capacity lists for each entry, the storage root's id or `fallback` for the download directory."
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
        description = "List the saved versions of one automation (id from list_automations): version number, trigger, condition, actions and when each was saved. To restore an older version, pass its trigger, condition and actions to update_automation, which saves them as a new version."
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
        description = "Reset one automation's version history (id from list_automations): every saved version but the one in force is deleted together with its runs, and the one in force becomes version 1; the runs of that version stay. Irreversible, so it asks first: a call without a `confirmation` code changes nothing and answers with a question for the person and a code; call again with confirmed=true and that code only after the person agreed. Refused with automation.version_reset_run_active while a run of an older version is still queued, running or waiting for a retry (list_automation_runs shows it). Answers with the automation and how many versions and runs were removed."
    )]
    pub async fn reset_automation_versions(
        &self,
        Parameters(params): Parameters<ResetAutomationVersionsParams>,
    ) -> McpToolResult {
        if let Some(question) = self.ask_first(
            "reset_automation_versions",
            params.confirmation.as_deref().filter(|_| params.confirmed),
            "Delete every saved version of this automation but the one in force, together with \
             the runs of the deleted versions, and make the one in force version 1.",
        ) {
            return question;
        }
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(reset) =
                automations::reset_automation_versions(State(self.state.clone()), Path(id)).await?;
            Ok(reset)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Ask, per active automation, whether a trigger would match it and whether its conditions would hold, without running anything. `trigger` is a word from get_automation_vocabulary; `package_id` (from list_packages) is the package the conditions are judged against. With `draft` (`trigger`, `condition`, optional `schedule`, `actions` and `automation_id`) only that automation is judged, saved or not, enabled or not; one not saved yet answers with the nil id. Each answer lists the actions a match would run and, for the `schedule` trigger, `next_run_at`."
    )]
    pub async fn dry_run_automations(
        &self,
        Parameters(params): Parameters<DryRunParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(object(&[
                ("trigger", params.trigger.into()),
                ("package_id", params.package_id.into()),
                ("draft", params.draft.into()),
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
        description = "List the destinations installed notification plugins provide: plugin_id (what a plugin target carries in config.plugin_id), name, version and the settings a target of it may hold in config.settings. A setting with choices takes one of them; one with no choices is free text: one line of at most 256 characters, or several (multiline: true) of at most 4096 together, no control characters but line breaks, refused otherwise with plugin.setting_text_invalid. Plex takes `section` (the library number; empty = every library) and Plex, Jellyfin and Emby `path_mapping`, one `rDownloader path=server path` per line: a finished package whose folder a line holds (the longest wins) is scanned alone, any other refreshes the whole library."
    )]
    pub async fn list_notification_destinations(&self) -> McpToolResult {
        let Json(destinations) =
            crate::notify_handlers::list_destinations(State(self.state.clone())).await;
        json_result(&destinations)
    }

    #[tool(
        description = "Try a routing rule's regular expression before writing it: whether it compiles (and the error if not), and for each of up to 50 sample texts whether and where it matches. With `replacement` it tries a package-name regex rule (find → replace, `$1`/`${name}` for groups, every match replaced) and answers what each sample becomes in `replaced`. A valid pattern also comes back as `structure`, the tree the regex editor draws as a diagram: nodes of a `kind` (sequence, alternation, group with `index`/`name`, repetition with `min`/`max`/`lazy`, literal `text`, class with `range`s and `negated`, digit/word_char/whitespace/any_char, start/end/word boundaries, flags) and their `children`; a pattern too deep or too large to draw carries `structure_error` `category_rule.regex_structure_limits` instead."
    )]
    pub async fn test_category_regex(
        &self,
        Parameters(params): Parameters<TestRegexParams>,
    ) -> McpToolResult {
        let result = async {
            let request = body(serde_json::json!({
                "pattern": params.pattern,
                "samples": params.samples,
                "replacement": params.replacement,
            }))?;
            let Json(answer) = crate::regex_tester::test_category_rule_regex(Json(request)).await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }
}
