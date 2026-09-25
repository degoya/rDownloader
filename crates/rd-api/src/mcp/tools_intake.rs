//! MCP tools for the scheduled intake sources: subscriptions and monitored livestreams.
//!
//! Both bodies are trees — filters and a backlog policy here, a recording policy there — so
//! these tools take the REST body as a JSON object rather than restating its grammar in a
//! second schema language. It is still the REST handler that validates it, and
//! [`super::error::without_credentials`] refuses a body that tries to smuggle an indexer API
//! key through the passthrough.
//!
//! A script subscription (RD-130-19) is refused outright, by [`refuse_script`]: it starts code
//! on this machine, and the owner's line is that no tool does.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, from_definition, parse_id, respond},
    params_config::IdParams,
    params_delivery::{DefinitionParams, UpdateDefinitionParams},
};

/// Refuses a tool call that would create, change, switch on or run a script subscription.
///
/// `requested` is the kind a body asks for, `stored` the subscription a call touches. Checked
/// by the tools rather than left to the scope check in the handler: an administrator's token
/// holds that scope, and the line is about the tool, not about who holds it.
pub(super) async fn refuse_script(
    state: &crate::AppState,
    requested: Option<rd_core::SubscriptionKind>,
    stored: Option<rd_core::SubscriptionId>,
) -> Result<(), crate::ApiError> {
    let stored = match stored {
        Some(id) => state
            .database
            .subscription(id)
            .await?
            .map(|subscription| subscription.kind),
        None => None,
    };
    if requested == Some(rd_core::SubscriptionKind::Script)
        || stored == Some(rd_core::SubscriptionKind::Script)
    {
        return Err(crate::ApiError::forbidden(
            "subscription.script_via_mcp",
            "A script subscription starts code on this machine and is not available to tools",
        ));
    }
    Ok(())
}

#[tool_router(router = intake_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List the subscriptions that poll feeds, channels, playlists, galleries, indexers and scripts. Stored indexer keys are never included."
    )]
    pub async fn list_subscriptions(&self) -> McpToolResult {
        respond(
            crate::subscription_handlers::list_subscriptions(State(self.state.clone()))
                .await
                .map(|rows| rows.0),
        )
    }

    #[tool(
        description = "Create a subscription. `definition` is the REST body of POST /api/v1/subscriptions: name, url, kind (rss|newznab|youtube_channel|…), enabled, mode, category_id, priority, interval_seconds, filters, backlog, category_map, source_categories, every_release, view (list|cards: how the LinkGrabber draws the pending hits), autoplay (the card slider turns its pages on its own), card_ratio (1:1|3:2|16:9|4:3|2:1, default 2:1: the shape of a card's image area; anything else is refused with subscription.card_ratio_unknown). An indexer API key is entered in the web UI and is refused here, and so is kind script (subscription.script_via_mcp): it runs code on the machine."
    )]
    pub async fn create_subscription(
        &self,
        Parameters(params): Parameters<DefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let request: crate::subscription_handlers::SubscriptionRequest =
                from_definition(params.definition)?;
            refuse_script(&self.state, Some(request.kind), None).await?;
            Ok(crate::subscription_handlers::create_subscription(
                State(self.state.clone()),
                None,
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
        description = "Replace one subscription. `definition` is the same body as create; it is a replacement, so send every field you want to keep. A stored indexer key survives an edit that does not mention one. A script subscription can be neither made nor changed here (subscription.script_via_mcp)."
    )]
    pub async fn update_subscription(
        &self,
        Parameters(params): Parameters<UpdateDefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request: crate::subscription_handlers::SubscriptionRequest =
                from_definition(params.definition)?;
            refuse_script(&self.state, Some(request.kind), Some(id)).await?;
            Ok(crate::subscription_handlers::update_subscription(
                State(self.state.clone()),
                None,
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
        description = "Delete one subscription, its item history and its stored indexer key. Destructive; downloads it already produced are kept."
    )]
    pub async fn delete_subscription(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            crate::subscription_handlers::delete_subscription(
                State(self.state.clone()),
                AxumPath(id),
            )
            .await?;
            Ok(serde_json::json!({ "deleted": params.id }))
        }
        .await;
        respond(result)
    }

    #[tool(description = "List the livestream channels being monitored for recordings.")]
    pub async fn list_stream_channels(&self) -> McpToolResult {
        respond(
            crate::stream_handlers::list_stream_channels(State(self.state.clone()))
                .await
                .map(|rows| rows.0),
        )
    }

    #[tool(
        description = "Add a livestream channel to monitor. `definition` is the REST body of POST /api/v1/streams/channels: url, name, quality, category_id, enabled, recording."
    )]
    pub async fn create_stream_channel(
        &self,
        Parameters(params): Parameters<DefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let request = from_definition(params.definition)?;
            Ok(crate::stream_handlers::create_stream_channel(
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
        description = "Replace one monitored livestream channel. `definition` is the same body as create; it is a replacement, so send every field you want to keep."
    )]
    pub async fn update_stream_channel(
        &self,
        Parameters(params): Parameters<UpdateDefinitionParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            let request = from_definition(params.definition)?;
            Ok(crate::stream_handlers::update_stream_channel(
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
        description = "Stop monitoring one livestream channel and remove it. Destructive; recordings already made are kept."
    )]
    pub async fn delete_stream_channel(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id = parse_id(&params.id)?;
            crate::stream_handlers::delete_stream_channel(State(self.state.clone()), AxumPath(id))
                .await?;
            Ok(serde_json::json!({ "deleted": params.id }))
        }
        .await;
        respond(result)
    }
}
