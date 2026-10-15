//! MCP tools for the LinkFilter rules (RD-1240-09): list, write, order, apply to the
//! LinkGrabber (with a dry run, RD-1310-01), and show the links they hid.
//!
//! Every tool builds the REST body and hands it to the REST handler, so validation and the
//! stable error codes are the ones the web UI gets.

use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, parse_ids, respond},
    params_config::{IdParams, clearing, merged},
    params_link_filters::{
        ApplyLinkFiltersParams, CreateLinkFilterParams, ReorderLinkFiltersParams,
        UnhideCandidatesParams, UpdateLinkFilterParams,
    },
};
use crate::{
    ApiError,
    link_filter_handlers::{
        CandidateUnhideRequest, LinkFilterApplyQuery, LinkFilterReorderRequest,
        LinkFilterRuleRequest,
    },
};

#[tool_router(router = link_filters_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List the LinkFilter rules in evaluation order. When a link arrives in the LinkGrabber, and again for that link when its online check ends, the first enabled rule whose conditions all hold decides: hide it (kept, shown only with the list's \"Show hidden\" switch, and left behind when its package is queued), accept it (kept, no later rule asked), route it into a package and/or category, or delete it (removed with a package it leaves empty)."
    )]
    pub async fn list_link_filters(&self) -> McpToolResult {
        respond(
            crate::link_filter_handlers::list_link_filter_rules(State(self.state.clone()))
                .await
                .map(|rules| rules.0),
        )
    }

    #[tool(
        description = "Create a LinkFilter rule at the end of the order. Conditions are combined and an empty one holds for every link: name_pattern (glob by default, or regex) on the file name, size_min/size_max in bytes (an unknown size never matches), extensions, hoster (subdomains included), source, and without_resolver (only links the online check found without an installed resolver, collector.check_no_resolver; never before the check). action is hide, accept, route or delete; route needs package_name or category_id; delete removes the link and an emptied package without asking. It decides for links arriving from now on, and once more for a link when its check ends; apply_link_filters decides the LinkGrabber's links again."
    )]
    pub async fn create_link_filter(
        &self,
        Parameters(params): Parameters<CreateLinkFilterParams>,
    ) -> McpToolResult {
        let result = async {
            let request = LinkFilterRuleRequest {
                name: params.name,
                enabled: params.enabled.unwrap_or(true),
                name_pattern: params.name_pattern,
                name_syntax: params.name_syntax.map(Into::into).unwrap_or_default(),
                size_min: params.size_min,
                size_max: params.size_max,
                extensions: params.extensions.unwrap_or_default(),
                hoster: params.hoster,
                source: params.source.map(Into::into),
                without_resolver: params.without_resolver.unwrap_or(false),
                action: params.action.into(),
                package_name: params.package_name,
                category_id: params.category_id.as_deref().map(parse_id).transpose()?,
            };
            Ok(crate::link_filter_handlers::create_link_filter_rule(
                State(self.state.clone()),
                Json(request),
            )
            .await?
            .1
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change one LinkFilter rule. Only the fields you pass are changed; conditions to drop go in `clear`, and without_resolver is switched with true or false. Its place in the order stays (reorder_link_filters moves it). Setting action to delete makes the rule remove the links it matches from then on."
    )]
    pub async fn update_link_filter(
        &self,
        Parameters(params): Parameters<UpdateLinkFilterParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::LinkFilterRuleId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .list_link_filter_rules()
                .await?
                .into_iter()
                .find(|rule| rule.id == id)
                .ok_or_else(|| {
                    ApiError::not_found("link_filter.not_found", "LinkFilter rule not found")
                })?;
            let cleared = clearing(
                params.clear.as_ref(),
                &[
                    "name_pattern",
                    "size_min",
                    "size_max",
                    "hoster",
                    "source",
                    "package_name",
                    "category_id",
                ],
            )?;
            let category_id = params.category_id.as_deref().map(parse_id).transpose()?;
            let request = LinkFilterRuleRequest {
                name: params.name.unwrap_or(current.name),
                enabled: params.enabled.unwrap_or(current.enabled),
                name_pattern: merged(
                    &cleared,
                    "name_pattern",
                    params.name_pattern,
                    current.name_pattern,
                ),
                name_syntax: params.name_syntax.map_or(current.name_syntax, Into::into),
                size_min: merged(&cleared, "size_min", params.size_min, current.size_min),
                size_max: merged(&cleared, "size_max", params.size_max, current.size_max),
                extensions: params.extensions.unwrap_or(current.extensions),
                hoster: merged(&cleared, "hoster", params.hoster, current.hoster),
                source: merged(
                    &cleared,
                    "source",
                    params.source.map(Into::into),
                    current.source,
                ),
                without_resolver: params.without_resolver.unwrap_or(current.without_resolver),
                action: params.action.map_or(current.action, Into::into),
                package_name: merged(
                    &cleared,
                    "package_name",
                    params.package_name,
                    current.package_name,
                ),
                category_id: merged(&cleared, "category_id", category_id, current.category_id),
            };
            Ok(crate::link_filter_handlers::update_link_filter_rule(
                State(self.state.clone()),
                AxumPath(id),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete one LinkFilter rule. The links it hid in the LinkGrabber are shown again; nothing in the downloads changes."
    )]
    pub async fn delete_link_filter(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::LinkFilterRuleId = parse_id(&params.id)?;
            Ok(crate::link_filter_handlers::delete_link_filter_rule(
                State(self.state.clone()),
                AxumPath(id),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Set the order the LinkFilter rules are asked in: the ids given come first, in that order, the others follow in theirs. Answers the rules in their new order."
    )]
    pub async fn reorder_link_filters(
        &self,
        Parameters(params): Parameters<ReorderLinkFiltersParams>,
    ) -> McpToolResult {
        let result = async {
            let request = LinkFilterReorderRequest {
                ids: parse_ids(&params.ids)?,
            };
            Ok(crate::link_filter_handlers::reorder_link_filter_rules(
                State(self.state.clone()),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Decide every link in the LinkGrabber again by the LinkFilter rules as they are now: hide what a hide rule matches, show what no hide rule matches any more, file what a route rule matches and delete what a delete rule matches. Links already queued are not touched; hidden links are kept. With dry_run nothing changes and the counts say what would: use it to tell the person how many links a delete rule removes before applying. Answers how many links were hidden, shown, routed and deleted."
    )]
    pub async fn apply_link_filters(
        &self,
        Parameters(params): Parameters<ApplyLinkFiltersParams>,
    ) -> McpToolResult {
        respond(
            crate::link_filter_handlers::apply_link_filters(
                State(self.state.clone()),
                Query(LinkFilterApplyQuery {
                    dry_run: params.dry_run.unwrap_or(false),
                }),
            )
            .await
            .map(|outcome| outcome.0),
        )
    }

    #[tool(
        description = "Show LinkGrabber links a LinkFilter rule hid (ids from list_candidates, where a hidden link carries hidden_by_filter), so that queueing their package takes them too. Applying the rules again hides them again. Answers how many were hidden."
    )]
    pub async fn unhide_candidates(
        &self,
        Parameters(params): Parameters<UnhideCandidatesParams>,
    ) -> McpToolResult {
        let result = async {
            let request = CandidateUnhideRequest {
                candidate_ids: parse_ids(&params.candidate_ids)?,
            };
            Ok(crate::link_filter_handlers::unhide_candidates(
                State(self.state.clone()),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }
}
