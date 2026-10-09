//! MCP tools for notification destinations and the rules that route events to them.
//!
//! A destination's webhook secret, SMTP password or apprise URL stays in the vault: no tool
//! here takes one, and an update carries the stored one over untouched. A destination's address
//! is answered without its path and query (RD-1190-21), where Slack, Discord, Teams and most
//! self-hosted webhooks keep the key that lets anybody post; passed back unchanged, the masked
//! address keeps the stored one.

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
        description = "List the notification destinations (webhook, SMTP, apprise, plugin). Stored secrets are never included, and an address shows only its scheme and host: its path and query are [redacted], because a webhook keeps its key there. The full address is in the web UI."
    )]
    pub async fn list_notification_targets(&self) -> McpToolResult {
        respond(
            crate::notify_handlers::list_targets(State(self.state.clone()))
                .await
                .map(|targets| targets.0.into_iter().map(shown).collect::<Vec<_>>()),
        )
    }

    #[tool(
        description = "Create a notification destination. Metadata only: a webhook secret, SMTP password or apprise URL is entered in the web UI, because no tool accepts one. `config.executable`, the program an apprise destination runs, needs the administration permission (api:admin); without it the call is refused with auth.scope_insufficient. Leave it out to use the apprise found in the vendor folder or on PATH. Names are unique: a name another target has is refused with notification.name_taken."
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
        respond(result.map(shown))
    }

    #[tool(
        description = "Change one notification destination. The stored secret is kept; only the fields you pass are changed, and an endpoint passed back as list_notification_targets showed it ([redacted] path) keeps the stored address. Setting or changing `config.executable`, the program an apprise destination runs, needs the administration permission (api:admin); a path an administrator stored may be passed back unchanged. A name another target has is refused with notification.name_taken."
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
                // The address this tool answered with stands for the stored one.
                endpoint: match params.endpoint {
                    Some(endpoint) if endpoint != shown_endpoint(&current.endpoint) => endpoint,
                    _ => current.endpoint,
                },
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
        respond(result.map(shown))
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
        description = "Create a notification rule: which events, from which category and above which severity, go to one destination. Besides the queue events (among them usenet_job_hopeless: a Usenet download given up as beyond repair, and stop_mark_reached: the queue paused at its stop mark) there are operational ones: backup_failed, backup_verify_failed, update_available, plugin_update_available, plugin_update_failed, account_expiring, account_invalid, usenet_quota_reached."
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

/// A destination as a tool answers with it: its address without path and query.
fn shown(mut target: rd_notify::NotificationTarget) -> rd_notify::NotificationTarget {
    target.endpoint = shown_endpoint(&target.endpoint);
    target
}

/// What a tool shows of a destination's address (RD-1190-21). An address with a host loses its
/// path and query; an SMTP `host:port` and an apprise scheme are no such address and stay.
fn shown_endpoint(endpoint: &str) -> String {
    url::Url::parse(endpoint)
        .ok()
        .filter(url::Url::has_host)
        .and_then(|address| super::webhook_mask::mask_path(&address))
        .unwrap_or_else(|| endpoint.to_owned())
}

#[cfg(test)]
mod tests {
    use super::shown_endpoint;

    #[test]
    fn a_destination_address_shows_scheme_and_host_only() {
        assert_eq!(
            shown_endpoint("https://chat.example.org/hooks/a1b2c3"),
            "https://chat.example.org/[redacted]"
        );
        assert_eq!(
            shown_endpoint("https://ntfy.example.org"),
            "https://ntfy.example.org"
        );
        assert_eq!(
            shown_endpoint("smtp.example.org:587"),
            "smtp.example.org:587"
        );
        assert_eq!(shown_endpoint("tgram"), "tgram");
    }
}
