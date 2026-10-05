//! Notification targets, rules, the delivery history and the test action (RD-050-14).

use axum::{
    Json,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
};
use rd_api_core::input_checks::{TextLimit, required_text};
use rd_notify::{NotificationEvent, Severity, TargetKind};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::{AppState, error::ApiError};

/// Highest number of history rows one request may pull.
const MAX_HISTORY: u32 = 500;

#[derive(Deserialize, ToSchema)]
pub struct NotificationTargetRequest {
    pub name: String,
    pub kind: TargetKind,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Webhook: the URL. SMTP: the host. Apprise: the service scheme, for display only.
    pub endpoint: String,
    #[serde(default)]
    pub config: serde_json::Value,
    /// Webhook signing secret, SMTP password or the full apprise target URL. Write-only:
    /// it goes straight into the vault and is never returned.
    #[serde(default)]
    #[schema(write_only)]
    pub secret: Option<String>,
    /// Removes the stored secret; an omitted `secret` alone keeps it.
    #[serde(default)]
    pub clear_secret: bool,
}

const fn default_enabled() -> bool {
    true
}

#[derive(Deserialize, ToSchema)]
pub struct NotificationRuleRequest {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub target_id: rd_core::NotificationTargetId,
    /// Empty means every event.
    #[serde(default)]
    pub events: Vec<NotificationEvent>,
    #[serde(default)]
    pub category_id: Option<rd_core::CategoryId>,
    #[serde(default = "default_severity")]
    pub min_severity: Severity,
}

const fn default_severity() -> Severity {
    Severity::Info
}

#[derive(Deserialize, IntoParams)]
pub struct HistoryQuery {
    /// Newest rows to return (1–500).
    pub limit: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct TargetTestResponse {
    pub ok: bool,
    pub status: Option<u16>,
    /// Redacted excerpt of what the target answered; never contains a secret.
    pub detail: Option<String>,
}

/// One installed notification-destination plugin, for the target editor.
#[derive(Serialize, ToSchema)]
pub struct NotificationDestination {
    /// The value a target stores in `config.plugin_id`.
    pub plugin_id: String,
    /// Default display name. The interface prefers the plugin's own localised name.
    pub name: String,
    pub version: String,
    /// The plugin's message namespace: a setting's label is its code
    /// `<slug>.setting.<name>`, a choice's `<slug>.choice.<value>`.
    pub slug: String,
    /// What a target of this destination may be set to, stored in `config.settings`
    /// (RD-170-09).
    pub settings: Vec<NotificationDestinationSetting>,
}

/// One setting a notification destination offers.
#[derive(Serialize, ToSchema)]
pub struct NotificationDestinationSetting {
    /// The key in `config.settings`.
    pub name: String,
    /// Every value the setting accepts, in the order to offer them.
    pub choices: Vec<String>,
    /// What applies while the target leaves the setting alone; absent means it may stay unset.
    pub default: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/notifications/destinations", tag = "notifications", responses((status = 200, body = [NotificationDestination])))]
pub async fn list_destinations(
    State(state): State<AppState>,
) -> Json<Vec<NotificationDestination>> {
    Json(
        state
            .notifications
            .notifiers()
            .await
            .list()
            .into_iter()
            .map(|destination| NotificationDestination {
                plugin_id: destination.plugin_id,
                name: destination.name,
                version: destination.version,
                slug: destination.slug,
                settings: destination
                    .settings
                    .into_iter()
                    .map(|setting| NotificationDestinationSetting {
                        name: setting.name,
                        choices: setting.choices,
                        default: setting.default,
                    })
                    .collect(),
            })
            .collect(),
    )
}

#[utoipa::path(get, path = "/api/v1/notifications/targets", tag = "notifications", responses((status = 200, body = [rd_notify::NotificationTarget])))]
pub async fn list_targets(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_notify::NotificationTarget>>, ApiError> {
    Ok(Json(state.database.list_notification_targets().await?))
}

#[utoipa::path(post, path = "/api/v1/notifications/targets", tag = "notifications", request_body = NotificationTargetRequest, responses((status = 201, body = rd_notify::NotificationTarget)))]
pub async fn create_target(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    Json(request): Json<NotificationTargetRequest>,
) -> Result<(StatusCode, Json<rd_notify::NotificationTarget>), ApiError> {
    let target = save_target(&state, None, holds_admin(granted), request).await?;
    Ok((StatusCode::CREATED, Json(target)))
}

#[utoipa::path(put, path = "/api/v1/notifications/targets/{id}", tag = "notifications", params(("id" = rd_core::NotificationTargetId, Path)), request_body = NotificationTargetRequest, responses((status = 200, body = rd_notify::NotificationTarget), (status = 404)))]
pub async fn update_target(
    State(state): State<AppState>,
    granted: Option<axum::Extension<crate::auth::Granted>>,
    AxumPath(id): AxumPath<rd_core::NotificationTargetId>,
    Json(request): Json<NotificationTargetRequest>,
) -> Result<Json<rd_notify::NotificationTarget>, ApiError> {
    Ok(Json(
        save_target(&state, Some(id), holds_admin(granted), request).await?,
    ))
}

fn holds_admin(granted: Option<axum::Extension<crate::auth::Granted>>) -> bool {
    granted.is_some_and(|axum::Extension(granted)| granted.holds(rd_core::Scope::Admin))
}

#[utoipa::path(delete, path = "/api/v1/notifications/targets/{id}", tag = "notifications", params(("id" = rd_core::NotificationTargetId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn delete_target(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::NotificationTargetId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    let secret_ref = state
        .database
        .delete_notification_target(id)
        .await
        .map_err(|error| not_found(&error, "notification.target_not_found"))?;
    // The vault entry goes with the target; leaving it would keep a credential nobody can
    // reach any more.
    if let Some(reference) = secret_ref
        && let Err(error) = state.secrets.remove(&reference).await
    {
        tracing::warn!(%error, "target secret could not be removed");
    }
    Ok(Json(crate::dto::MessageResponse::new(
        "notification.target_deleted",
        "Notification target deleted",
    )))
}

/// Sends one message to the target right now, so a wrong address surfaces here instead of
/// on the next finished download.
#[utoipa::path(post, path = "/api/v1/notifications/targets/{id}/test", tag = "notifications", params(("id" = rd_core::NotificationTargetId, Path)), responses((status = 200, body = TargetTestResponse), (status = 404)))]
pub async fn test_target(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::NotificationTargetId>,
) -> Result<Json<TargetTestResponse>, ApiError> {
    let target = state
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
    let attempt = state.notifications.test_target(&target).await;
    Ok(Json(TargetTestResponse {
        ok: attempt.ok,
        status: attempt.status,
        detail: attempt.excerpt,
    }))
}

#[utoipa::path(get, path = "/api/v1/notifications/rules", tag = "notifications", responses((status = 200, body = [rd_notify::NotificationRule])))]
pub async fn list_rules(
    State(state): State<AppState>,
) -> Result<Json<Vec<rd_notify::NotificationRule>>, ApiError> {
    Ok(Json(state.database.list_notification_rules().await?))
}

#[utoipa::path(post, path = "/api/v1/notifications/rules", tag = "notifications", request_body = NotificationRuleRequest, responses((status = 201, body = rd_notify::NotificationRule)))]
pub async fn create_rule(
    State(state): State<AppState>,
    Json(request): Json<NotificationRuleRequest>,
) -> Result<(StatusCode, Json<rd_notify::NotificationRule>), ApiError> {
    let rule = save_rule(&state, None, request).await?;
    Ok((StatusCode::CREATED, Json(rule)))
}

#[utoipa::path(put, path = "/api/v1/notifications/rules/{id}", tag = "notifications", params(("id" = rd_core::NotificationRuleId, Path)), request_body = NotificationRuleRequest, responses((status = 200, body = rd_notify::NotificationRule), (status = 404)))]
pub async fn update_rule(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::NotificationRuleId>,
    Json(request): Json<NotificationRuleRequest>,
) -> Result<Json<rd_notify::NotificationRule>, ApiError> {
    Ok(Json(save_rule(&state, Some(id), request).await?))
}

#[utoipa::path(delete, path = "/api/v1/notifications/rules/{id}", tag = "notifications", params(("id" = rd_core::NotificationRuleId, Path)), responses((status = 200, body = crate::dto::MessageResponse), (status = 404)))]
pub async fn delete_rule(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<rd_core::NotificationRuleId>,
) -> Result<Json<crate::dto::MessageResponse>, ApiError> {
    state
        .database
        .delete_notification_rule(id)
        .await
        .map_err(|error| not_found(&error, "notification.rule_not_found"))?;
    Ok(Json(crate::dto::MessageResponse::new(
        "notification.rule_deleted",
        "Notification rule deleted",
    )))
}

#[utoipa::path(get, path = "/api/v1/notifications/deliveries", tag = "notifications", params(HistoryQuery), responses((status = 200, body = [rd_notify::Delivery])))]
pub async fn list_deliveries(
    State(state): State<AppState>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<rd_notify::Delivery>>, ApiError> {
    let limit = query
        .limit
        .unwrap_or(crate::notify_service::DEFAULT_HISTORY)
        .clamp(1, MAX_HISTORY);
    Ok(Json(
        state.database.list_notification_deliveries(limit).await?,
    ))
}

/// Saves a target on a caller's behalf, for the REST routes and the MCP tools alike.
///
/// `holds_admin` is the caller's own grant: setting or changing `config.executable`, the
/// program an apprise target starts, costs `api:admin` like the program paths among the
/// settings (`privileged_change`; audit 2026-10-05, S1). The MCP tools used to call the route
/// handlers, which is why they pass it here rather than through a request extension.
pub async fn save_target(
    state: &AppState,
    id: Option<rd_core::NotificationTargetId>,
    holds_admin: bool,
    request: NotificationTargetRequest,
) -> Result<rd_notify::NotificationTarget, ApiError> {
    let name = required_text(
        &request.name,
        TextLimit::Chars(100),
        "notification.name_invalid",
        "A target name must be between 1 and 100 characters",
    )?;
    let endpoint = required_text(
        &request.endpoint,
        TextLimit::Unbounded,
        "notification.endpoint_invalid",
        "A target needs an endpoint",
    )?;
    if request.kind == TargetKind::Plugin {
        // Refused now rather than at delivery time: a target naming a plugin nobody installed,
        // or a destination the host would refuse to send to (RD-130-15), would sit in the list
        // looking configured and fail on every event.
        let plugin_id = request
            .config
            .get("plugin_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        match state
            .notifications
            .notifiers()
            .await
            .check_destination(plugin_id, &endpoint)
        {
            None => {
                return Err(ApiError::bad_request(
                    "notification.plugin_unknown",
                    "No installed notification destination has that id",
                ));
            }
            Some(Err(failure)) => {
                return Err(ApiError::bad_request_owned(
                    failure
                        .code
                        .unwrap_or_else(|| "notification.endpoint_invalid".to_owned()),
                    failure.message,
                ));
            }
            Some(Ok(())) => {}
        }
        // The same for its settings (RD-170-09): a name the destination does not declare, or a
        // value it does not offer, would otherwise be dropped silently on every delivery.
        let checked = match rd_plugin_host::extension::settings_from_config(&request.config) {
            Ok(settings) => state
                .notifications
                .notifiers()
                .await
                .check_settings(plugin_id, &settings)
                .unwrap_or(Ok(())),
            Err(failure) => Err(failure),
        };
        if let Err(failure) = checked {
            return Err(ApiError::bad_request_owned(
                failure
                    .code
                    .unwrap_or_else(|| "plugin.setting_invalid".to_owned()),
                failure.message,
            ));
        }
    }
    if request.kind == TargetKind::Webhook {
        let url = url::Url::parse(&endpoint).map_err(|_| {
            ApiError::bad_request(
                "notification.endpoint_invalid",
                "The webhook URL is not valid",
            )
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ApiError::bad_request(
                "notification.endpoint_invalid",
                "A webhook must use http or https",
            ));
        }
    }
    let stored = state
        .database
        .list_notification_targets()
        .await?
        .into_iter()
        .find(|target| Some(target.id) == id);
    // Decided before the secret is written, so a refused save leaves no vault entry behind.
    let approved = rd_notify::check_executable(
        &request.config,
        stored
            .as_ref()
            .map(|target| (&target.config, target.secret_ref.as_deref())),
        holds_admin,
    )
    .map_err(|rd_notify::ExecutableNeedsAdmin| {
        ApiError::forbidden(
            "auth.scope_insufficient",
            "Naming the program a notification target runs requires the administration scope",
        )
        .with_param("scope", rd_core::Scope::Admin.as_str())
        .with_param("setting", "executable")
    })?;
    let secret_ref = match request.secret.as_deref().map(str::trim) {
        Some(value) if !value.is_empty() => Some(state.secrets.put_string(value.to_owned()).await?),
        _ => None,
    };
    let stale = stored.and_then(|target| target.secret_ref);
    // The seal binds the path to the reference the target keeps after this save, as the store
    // decides it; whatever the client sent under the seal's key is replaced.
    let kept_ref = if request.clear_secret {
        None
    } else {
        secret_ref.as_deref().or(stale.as_deref())
    };
    let mut config = request.config;
    rd_notify::seal_executable(&mut config, approved, kept_ref);
    let saved = state
        .database
        .upsert_notification_target(
            id,
            rd_db::NewNotificationTarget {
                name,
                kind: request.kind,
                enabled: request.enabled,
                endpoint,
                config,
                secret_ref: secret_ref.clone(),
                clear_secret: request.clear_secret,
            },
        )
        .await
        .map_err(|error| not_found(&error, "notification.target_not_found"))?;
    // A replaced or cleared secret leaves the old vault entry behind; drop it.
    if let Some(stale) = stale
        && (secret_ref.is_some() || request.clear_secret)
        && let Err(error) = state.secrets.remove(&stale).await
    {
        tracing::warn!(%error, "stale target secret could not be removed");
    }
    Ok(saved)
}

async fn save_rule(
    state: &AppState,
    id: Option<rd_core::NotificationRuleId>,
    request: NotificationRuleRequest,
) -> Result<rd_notify::NotificationRule, ApiError> {
    let name = required_text(
        &request.name,
        TextLimit::Chars(100),
        "notification.name_invalid",
        "A rule name must be between 1 and 100 characters",
    )?;
    let targets = state.database.list_notification_targets().await?;
    if !targets.iter().any(|target| target.id == request.target_id) {
        return Err(ApiError::bad_request(
            "notification.target_not_found",
            "The rule references a target that does not exist",
        ));
    }
    state
        .database
        .upsert_notification_rule(
            id,
            rd_db::NewNotificationRule {
                name,
                enabled: request.enabled,
                target_id: request.target_id,
                events: request.events,
                category_id: request.category_id,
                min_severity: request.min_severity,
            },
        )
        .await
        .map_err(|error| not_found(&error, "notification.rule_not_found"))
}

fn not_found(error: &anyhow::Error, code: &'static str) -> ApiError {
    crate::error_codes::store_not_found(error, code, "Not found")
}
