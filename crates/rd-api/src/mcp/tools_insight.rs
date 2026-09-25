//! MCP tools for what the installation can tell you about itself (RD-120-29).
//!
//! Four surfaces that arrived after the toolbox was written and were checked rather than
//! assumed: the transfer statistics, the log store, the audit log and the site-rule catalogue.
//! They share a shape -- a caller asking why something happened, or whether it happened at
//! all -- and they are the surfaces where an assistant is most obviously better than a person
//! scrolling, because the filter arguments are exactly what a question already contains.
//!
//! What is deliberately not here: approving a diagnostic bundle, which is the person's approval
//! of what goes in (its preview is in `super::tools_operations` since RD-120-55). Writing a site
//! rule, left out here at first, is `super::tools_site_rules` since RD-120-32; the reasons in
//! full are in `super::coverage`.
//!
//! None of these answers carries a credential. The audit log names actors by id and the log
//! store holds messages the service wrote with the redaction `rd-diagnostics` applies on the
//! way in; `crates/rd-api/tests/mcp.rs` walks the answers looking for one anyway.

use axum::extract::{Path as AxumPath, Query, State};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
    params_insight::{
        AuditQueryToolParams, DataClearToolParams, LogQueryToolParams, SiteRuleGroupSwitchParams,
        SiteRuleSwitchParams, TransferStatsParams,
    },
};
use crate::{data_reset_handlers::DataClearRequest, site_rules_dto::SiteRuleSwitchRequest};

#[tool_router(router = insight_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Transfer statistics over a range: per-bucket volume, totals by kind and by provider, and the all-time figures. `range` is day, week, month or year; day gives hourly buckets, the rest daily ones."
    )]
    pub async fn get_transfer_stats(
        &self,
        Parameters(params): Parameters<TransferStatsParams>,
    ) -> McpToolResult {
        respond(
            crate::stats_handlers::transfer_stats(
                State(self.state.clone()),
                Query(crate::stats_handlers::TransferStatsParams {
                    range: params.range.unwrap_or_default().into(),
                }),
            )
            .await
            .map(|response| response.0),
        )
    }

    #[tool(
        description = "Read the service log. Every argument is a filter and all are optional: level (this one and more severe), component prefix, exact stable code, correlation_id, a case-insensitive search in the message, since/until as RFC 3339, and limit (1-500). `full_page` true means there are older records; page back with before_id."
    )]
    pub async fn list_log_records(
        &self,
        Parameters(params): Parameters<LogQueryToolParams>,
    ) -> McpToolResult {
        respond(
            crate::diagnostics_handlers::list_log_records(
                State(self.state.clone()),
                Query(crate::diagnostics_dto::LogQueryParams {
                    level: params.level,
                    component: params.component,
                    code: params.code,
                    correlation_id: params.correlation_id,
                    search: params.search,
                    since: params.since,
                    until: params.until,
                    before_id: params.before_id,
                    limit: params.limit,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }

    #[tool(
        description = "Read the audit log: who did what, when, and whether it succeeded. Every argument is an optional filter -- action, outcome (success|failure), actor_kind, actor_id, target_kind, target_id, trace_id, since/until as RFC 3339, limit (1-500). The answer lists every action word it knows, so an unfamiliar one can be looked up there."
    )]
    pub async fn list_audit_records(
        &self,
        Parameters(params): Parameters<AuditQueryToolParams>,
    ) -> McpToolResult {
        respond(
            crate::audit_handlers::list_audit_records(
                State(self.state.clone()),
                Query(crate::audit_dto::AuditQueryParams {
                    action: params.action,
                    outcome: params.outcome,
                    actor_kind: params.actor_kind,
                    actor_id: params.actor_id,
                    target_kind: params.target_kind,
                    target_id: params.target_id,
                    trace_id: params.trace_id,
                    since: params.since,
                    until: params.until,
                    before_id: params.before_id,
                    limit: params.limit,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }

    #[tool(
        description = "How many records a clear would remove right now: the service log, the audit log, the transfer statistics and the notification history (its finished deliveries only), each counted separately. Ask this before clearing anything, and say the numbers to the person."
    )]
    pub async fn get_data_reset_preview(&self) -> McpToolResult {
        respond(
            crate::data_reset_handlers::data_reset_preview(State(self.state.clone()))
                .await
                .map(|response| response.0),
        )
    }

    #[tool(
        description = "Empty the service log so a test run starts from nothing. Irreversible, and `confirmed` must be true. Downloads, packages and settings are untouched; the audit log and the statistics are left alone."
    )]
    pub async fn clear_log_records(
        &self,
        Parameters(params): Parameters<DataClearToolParams>,
    ) -> McpToolResult {
        respond(
            crate::data_reset_handlers::clear_log_records(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                axum::Json(DataClearRequest {
                    confirmed: params.confirmed,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }

    #[tool(
        description = "Empty the audit log. Irreversible, and `confirmed` must be true. The clear writes itself into the emptied log as its first entry -- when, by which credential, and how many records went -- so the log is never empty with nothing saying why. The service log and the statistics are left alone."
    )]
    pub async fn clear_audit_records(
        &self,
        Parameters(params): Parameters<DataClearToolParams>,
    ) -> McpToolResult {
        respond(
            crate::data_reset_handlers::clear_audit_records(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                axum::Json(DataClearRequest {
                    confirmed: params.confirmed,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }

    #[tool(
        description = "Empty the transfer statistics: the per-bucket history behind the charts and the all-time totals. Irreversible, and `confirmed` must be true. The queue itself is untouched, as are the service log and the audit log."
    )]
    pub async fn clear_transfer_stats(
        &self,
        Parameters(params): Parameters<DataClearToolParams>,
    ) -> McpToolResult {
        respond(
            crate::data_reset_handlers::clear_transfer_stats(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                axum::Json(DataClearRequest {
                    confirmed: params.confirmed,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }

    #[tool(
        description = "Empty the notification history: every delivered or failed delivery. Deliveries still queued or retrying stay, because they are notifications not yet sent. Irreversible, and `confirmed` must be true. Destinations, rules, the logs and the statistics are untouched."
    )]
    pub async fn clear_notification_deliveries(
        &self,
        Parameters(params): Parameters<DataClearToolParams>,
    ) -> McpToolResult {
        respond(
            crate::data_reset_handlers::clear_notification_deliveries(
                State(self.state.clone()),
                crate::audit::AuditContext::current(),
                axum::Json(DataClearRequest {
                    confirmed: params.confirmed,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }

    #[tool(
        description = "List the release-page rules that turn a link on a page into the files behind it: every rule of this installation, with its group, whether each is switched on, and whether it is active (a rule in a switched-off group is on but not active)."
    )]
    pub async fn list_site_rules(&self) -> McpToolResult {
        respond(
            crate::site_rules_handlers::list_site_rules(State(self.state.clone()))
                .await
                .map(|response| response.0),
        )
    }

    #[tool(
        description = "Switch one site rule on or off by its own identifier. Reversible, and it changes nothing about work already collected."
    )]
    pub async fn set_site_rule_enabled(
        &self,
        Parameters(params): Parameters<SiteRuleSwitchParams>,
    ) -> McpToolResult {
        respond(
            crate::site_rules_handlers::set_site_rule_enabled(
                State(self.state.clone()),
                AxumPath(params.id),
                axum::Json(SiteRuleSwitchRequest {
                    enabled: params.enabled,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }

    #[tool(
        description = "Switch a whole group of site rules on or off. A rule in a switched-off group stays on but is not active, so switching the group back restores what each rule was."
    )]
    pub async fn set_site_rule_group_enabled(
        &self,
        Parameters(params): Parameters<SiteRuleGroupSwitchParams>,
    ) -> McpToolResult {
        respond(
            crate::site_rules_handlers::set_site_rule_group_enabled(
                State(self.state.clone()),
                AxumPath(params.group),
                axum::Json(SiteRuleSwitchRequest {
                    enabled: params.enabled,
                }),
            )
            .await
            .map(|response| response.0),
        )
    }
}
