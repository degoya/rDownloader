//! MCP tools for what a subscription found and what happens to it: the review list, the poll
//! history, switching one on and off, polling now (RD-120-55).
//!
//! RD-120-29 left these out because a subscription polls on a schedule and the review is "a
//! list a person approves from". Neither is one of the owner's four marks: approving a hit
//! hands a link to the LinkGrabber exactly as the automatic mode does, and a poll is a read of
//! the indexer or feed like the scheduled one. What does meet a mark stays out -- the
//! capability probe, which is the indexer's key test (with a stored key) or takes a key in (with
//! a new one); see `super::coverage`.
//!
//! The items carry their archive password in clear (RD-104-04) and an indexer hit's address
//! carries the indexer's API key, because that is how the file is fetched later. The answer
//! goes through [`super::params_handling::public`] as every other row-returning tool does; the
//! key is masked on the way out of every tool, by `super::mask` (RD-120-57).

use axum::{
    Json,
    extract::{Path, Query, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, respond},
    params_config::IdParams,
    params_handling::{body, public},
    params_remaining::{
        ReviewParams, SubscriptionItemsParams, SubscriptionRunsParams, SubscriptionSwitchParams,
        object,
    },
};
use crate::subscription_handlers as subscriptions;

#[tool_router(router = subscription_review_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Count the hits waiting for review, per indexer subscription, and in total. An indexer with a count is one list_subscription_items has pending hits for."
    )]
    pub async fn get_subscription_review_summary(&self) -> McpToolResult {
        respond(
            subscriptions::subscription_review_summary(State(self.state.clone()))
                .await
                .map(|Json(answer)| answer),
        )
    }

    #[tool(
        description = "List one subscription's hits (id from list_subscriptions), one page at a time: title, address, size, when found, and state. `state` filters: pending (default: waiting for review), queued, dismissed, skipped or all. Item ids are what review_subscription_item takes."
    )]
    pub async fn list_subscription_items(
        &self,
        Parameters(params): Parameters<SubscriptionItemsParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let query = body(object(&[
                ("state", params.state.into()),
                ("limit", params.limit.into()),
                ("offset", params.offset.into()),
            ]))?;
            let Json(page) = subscriptions::list_subscription_item_page(
                State(self.state.clone()),
                Path(id),
                Query(query),
            )
            .await?;
            public(&page)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List one subscription's newest polls (id from list_subscriptions): when it started and finished, how many hits it found, accepted and skipped, and the (redacted) error of a failed one."
    )]
    pub async fn list_subscription_runs(
        &self,
        Parameters(params): Parameters<SubscriptionRunsParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let query = body(object(&[("limit", params.limit.into())]))?;
            let Json(runs) = subscriptions::list_subscription_runs(
                State(self.state.clone()),
                Path(id),
                Query(query),
            )
            .await?;
            Ok(runs)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Switch one subscription on or off (id from list_subscriptions). A switched-off subscription keeps its settings and history and stops polling. A script subscription can be switched neither on nor off here (subscription.script_via_mcp)."
    )]
    pub async fn set_subscription_enabled(
        &self,
        Parameters(params): Parameters<SubscriptionSwitchParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            // Switching a script subscription decides when code runs, which is the
            // administrator's alone over REST and nobody's here (RD-130-19).
            super::tools_intake::refuse_script(&self.state, None, Some(id)).await?;
            let id = Path(id);
            let state = State(self.state.clone());
            let Json(row) = if params.enabled {
                subscriptions::enable_subscription(state, None, id).await?
            } else {
                subscriptions::disable_subscription(state, None, id).await?
            };
            Ok(row)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Poll one subscription now, outside its schedule (id from list_subscriptions). Answers at once; the poll runs in the background, and list_subscription_runs shows how it went. A script subscription is refused (subscription.script_via_mcp): running it runs code on the machine."
    )]
    pub async fn poll_subscription(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            super::tools_intake::refuse_script(&self.state, None, Some(id)).await?;
            subscriptions::poll_subscription(State(self.state.clone()), None, Path(id)).await?;
            Ok(serde_json::json!({ "polling": params.id }))
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Decide one hit waiting for review (item id from list_subscription_items). state=queued hands it to the LinkGrabber through the same intake the automatic mode uses; dismissed sets it aside."
    )]
    pub async fn review_subscription_item(
        &self,
        Parameters(params): Parameters<ReviewParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(serde_json::json!({ "state": params.state }))?;
            subscriptions::set_subscription_item_state(
                State(self.state.clone()),
                Path(id),
                Json(request),
            )
            .await?;
            Ok(serde_json::json!({ "item": params.id, "state": params.state }))
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Decide every hit of one subscription that is waiting for review (id from list_subscriptions): state=queued or dismissed. Answers how many matched, how many changed and how many could not be handed over."
    )]
    pub async fn review_pending_subscription_items(
        &self,
        Parameters(params): Parameters<ReviewParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = body(serde_json::json!({ "state": params.state }))?;
            let Json(answer) = subscriptions::set_pending_subscription_items_state(
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
        description = "Clear one subscription's history (id from list_subscriptions): the settled hits and the poll runs. Hits still waiting for review are kept. Destructive."
    )]
    pub async fn clear_subscription_history(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let Json(answer) =
                subscriptions::clear_subscription_history(State(self.state.clone()), Path(id))
                    .await?;
            Ok(answer)
        }
        .await;
        respond(result)
    }
}
