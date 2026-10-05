//! MCP tools for notification destinations and the rules that route events to them.
//!
//! A destination's webhook secret, SMTP password or apprise URL stays in the vault: no tool
//! here takes one, and an update carries the stored one over untouched.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, respond, without_credentials},
    params_config::{IdParams, clearing, merged},
    params_delivery::{
        CreateNotificationRuleParams, CreateNotificationTargetParams, UpdateNotificationRuleParams,
        UpdateNotificationTargetParams,
    },
};
use crate::{
    ApiError,
    notify_handlers::{NotificationRuleRequest, NotificationTargetRequest},
};

#[tool_router(router = notify_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "List the notification destinations (webhook, SMTP, apprise, plugin). Stored secrets are never included."
    )]
    pub async fn list_notification_targets(&self) -> McpToolResult {
        respond(
            crate::notify_handlers::list_targets(State(self.state.clone()))
                .await
                .map(|targets| targets.0),
        )
    }

    #[tool(
        description = "Create a notification destination. Metadata only: a webhook secret, SMTP password or apprise URL is entered in the web UI, because no tool accepts one. `config.executable`, the program an apprise destination runs, needs the administration permission (api:admin); without it the call is refused with auth.scope_insufficient. Leave it out to use the apprise found in the vendor folder or on PATH."
    )]
    pub async fn create_notification_target(
        &self,
        Parameters(params): Parameters<CreateNotificationTargetParams>,
    ) -> McpToolResult {
        let result = async {
            let request = NotificationTargetRequest {
                name: params.name,
                kind: params.kind.into(),
                enabled: params.enabled.unwrap_or(true),
                endpoint: params.endpoint,
                // `config` is a free-form object stored and read back verbatim, so it is the
                // one place a credential could still arrive under a name the tool schema
                // never declared — and it would then be handed straight back by
                // `list_notification_targets`. Scanned like a passthrough body.
                config: params
                    .config
                    .map(without_credentials)
                    .transpose()?
                    .map_or(serde_json::Value::Null, serde_json::Value::Object),
                secret: None,
                clear_secret: false,
            };
            crate::notify_handlers::save_target(&self.state, None, holds_admin(), request).await
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change one notification destination. The stored secret is kept; only the fields you pass are changed. Setting or changing `config.executable`, the program an apprise destination runs, needs the administration permission (api:admin); a path an administrator stored may be passed back unchanged."
    )]
    pub async fn update_notification_target(
        &self,
        Parameters(params): Parameters<UpdateNotificationTargetParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::NotificationTargetId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .list_notification_targets()
                .await?
                .into_iter()
                .find(|target| target.id == id)
                .ok_or_else(|| {
                    ApiError::not_found(
                        "notification.target_not_found",
                        "Notification target not found",
                    )
                })?;
            let request = NotificationTargetRequest {
                name: params.name.unwrap_or(current.name),
                kind: params.kind.map_or(current.kind, Into::into),
                enabled: params.enabled.unwrap_or(current.enabled),
                endpoint: params.endpoint.unwrap_or(current.endpoint),
                config: params
                    .config
                    .map(without_credentials)
                    .transpose()?
                    .map_or(current.config, serde_json::Value::Object),
                secret: None,
                clear_secret: false,
            };
            crate::notify_handlers::save_target(&self.state, Some(id), holds_admin(), request).await
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete one notification destination and its stored secret. Destructive; rules pointing at it stop delivering."
    )]
    pub async fn delete_notification_target(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::NotificationTargetId = parse_id(&params.id)?;
            Ok(
                crate::notify_handlers::delete_target(State(self.state.clone()), AxumPath(id))
                    .await?
                    .0,
            )
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the rules deciding which events reach which notification destination."
    )]
    pub async fn list_notification_rules(&self) -> McpToolResult {
        respond(
            crate::notify_handlers::list_rules(State(self.state.clone()))
                .await
                .map(|rules| rules.0),
        )
    }

    #[tool(
        description = "Create a notification rule: which events, from which category and above which severity, go to one destination. Besides the queue events (among them usenet_job_hopeless: a Usenet download given up as beyond repair) there are operational ones: backup_failed, backup_verify_failed, update_available, plugin_update_available, plugin_update_failed, account_expiring, account_invalid, usenet_quota_reached."
    )]
    pub async fn create_notification_rule(
        &self,
        Parameters(params): Parameters<CreateNotificationRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let request = NotificationRuleRequest {
                name: params.name,
                enabled: params.enabled.unwrap_or(true),
                target_id: parse_id(&params.target_id)?,
                events: params
                    .events
                    .unwrap_or_default()
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                category_id: params.category_id.as_deref().map(parse_id).transpose()?,
                min_severity: params
                    .min_severity
                    .map_or(rd_notify::Severity::Info, Into::into),
            };
            Ok(
                crate::notify_handlers::create_rule(State(self.state.clone()), Json(request))
                    .await?
                    .1
                    .0,
            )
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change one notification rule. Only the fields you pass are changed; `clear` may drop the category restriction."
    )]
    pub async fn update_notification_rule(
        &self,
        Parameters(params): Parameters<UpdateNotificationRuleParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::NotificationRuleId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .list_notification_rules()
                .await?
                .into_iter()
                .find(|rule| rule.id == id)
                .ok_or_else(|| {
                    ApiError::not_found(
                        "notification.rule_not_found",
                        "Notification rule not found",
                    )
                })?;
            let cleared = clearing(params.clear.as_ref(), &["category_id"])?;
            let request = NotificationRuleRequest {
                name: params.name.unwrap_or(current.name),
                enabled: params.enabled.unwrap_or(current.enabled),
                target_id: match params.target_id {
                    Some(value) => parse_id(&value)?,
                    None => current.target_id,
                },
                events: params.events.map_or(current.events, |events| {
                    events.into_iter().map(Into::into).collect()
                }),
                category_id: merged(
                    &cleared,
                    "category_id",
                    params.category_id.as_deref().map(parse_id).transpose()?,
                    current.category_id,
                ),
                min_severity: params.min_severity.map_or(current.min_severity, Into::into),
            };
            Ok(crate::notify_handlers::update_rule(
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
        description = "Delete one notification rule. Destructive; the destination itself is kept."
    )]
    pub async fn delete_notification_rule(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::NotificationRuleId = parse_id(&params.id)?;
            Ok(
                crate::notify_handlers::delete_rule(State(self.state.clone()), AxumPath(id))
                    .await?
                    .0,
            )
        }
        .await;
        respond(result)
    }
}

/// Whether the caller of this tool call holds `api:admin`, which the program path of an apprise
/// destination costs on top of the tools' `api:config` (audit 2026-10-05, S1).
fn holds_admin() -> bool {
    super::granted_now().contains(&rd_core::Scope::Admin)
}
