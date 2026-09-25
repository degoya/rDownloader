//! Parameters for the notification, subscription, stream, automation and plugin tools.
//!
//! Three of these bodies are trees rather than records — an automation carries a condition
//! grammar and an action list, a subscription carries filters and a backlog policy, a stream
//! channel carries a recording policy. Mirroring those in a second schema language would mean
//! restating `rd-automation`'s vocabulary here and letting the two drift. Those tools take the
//! REST body as a JSON object instead, named in the tool description and validated by the same
//! handler; the flat ones are mirrored field by field, as in [`super::params_config`].

use rmcp::schemars;
use serde::Deserialize;

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TargetKindParam {
    /// Signed JSON posted to a URL.
    Webhook,
    /// Mail through a configured SMTP server.
    Smtp,
    /// The external `apprise` CLI (Telegram, Discord, Matrix, ntfy, …).
    Apprise,
    /// A signed notification-destination plugin, named by `config.plugin_id`.
    Plugin,
}

impl From<TargetKindParam> for rd_notify::TargetKind {
    fn from(value: TargetKindParam) -> Self {
        match value {
            TargetKindParam::Webhook => Self::Webhook,
            TargetKindParam::Smtp => Self::Smtp,
            TargetKindParam::Apprise => Self::Apprise,
            TargetKindParam::Plugin => Self::Plugin,
        }
    }
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SeverityParam {
    Info,
    Warning,
    Error,
}

impl From<SeverityParam> for rd_notify::Severity {
    fn from(value: SeverityParam) -> Self {
        match value {
            SeverityParam::Info => Self::Info,
            SeverityParam::Warning => Self::Warning,
            SeverityParam::Error => Self::Error,
        }
    }
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NotificationEventParam {
    PackageCompleted,
    PackageFailed,
    /// A storage root fell below its free-space threshold.
    StorageBlocked,
    /// The active bandwidth profile used up its traffic budget.
    BudgetExhausted,
    /// A captcha is waiting for a person.
    CaptchaWaiting,
    /// A queue-completion action is counting down.
    PowerPending,
}

impl From<NotificationEventParam> for rd_notify::NotificationEvent {
    fn from(value: NotificationEventParam) -> Self {
        match value {
            NotificationEventParam::PackageCompleted => Self::PackageCompleted,
            NotificationEventParam::PackageFailed => Self::PackageFailed,
            NotificationEventParam::StorageBlocked => Self::StorageBlocked,
            NotificationEventParam::BudgetExhausted => Self::BudgetExhausted,
            NotificationEventParam::CaptchaWaiting => Self::CaptchaWaiting,
            NotificationEventParam::PowerPending => Self::PowerPending,
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateNotificationTargetParams {
    pub name: String,
    pub kind: TargetKindParam,
    /// Webhook: the URL. SMTP: `host:port`. Apprise: the service scheme.
    pub endpoint: String,
    /// Settings without any secret: SMTP TLS mode, sender and recipients, webhook headers,
    /// `plugin_id` for a plugin destination.
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateNotificationTargetParams {
    pub id: String,
    pub name: Option<String>,
    pub kind: Option<TargetKindParam>,
    pub endpoint: Option<String>,
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateNotificationRuleParams {
    pub name: String,
    /// Destination this rule delivers to.
    pub target_id: String,
    /// Events the rule reacts to; empty means every event.
    pub events: Option<Vec<NotificationEventParam>>,
    /// Restricts the rule to one category.
    pub category_id: Option<String>,
    pub min_severity: Option<SeverityParam>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateNotificationRuleParams {
    pub id: String,
    pub name: Option<String>,
    pub target_id: Option<String>,
    pub events: Option<Vec<NotificationEventParam>>,
    pub category_id: Option<String>,
    pub min_severity: Option<SeverityParam>,
    pub enabled: Option<bool>,
    /// Fields to drop: category_id (the rule then matches every category).
    pub clear: Option<Vec<String>>,
}

/// A whole REST body handed through, for the three tree-shaped resources.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct DefinitionParams {
    /// The full request body, exactly as the matching REST endpoint documents it.
    pub definition: serde_json::Map<String, serde_json::Value>,
}

/// A whole REST body plus the id of the row it replaces.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateDefinitionParams {
    pub id: String,
    /// The full request body, exactly as the matching REST endpoint documents it. This is a
    /// replacement, not a patch: fields left out fall back to their documented default.
    pub definition: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct SetPluginEnabledParams {
    /// Plugin id as `list_configuration(section = "plugins")` reports it.
    pub id: String,
    pub enabled: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UninstallPluginParams {
    /// Plugin id as `list_configuration(section = "plugins")` reports it.
    pub id: String,
    /// The exact installed version to remove.
    pub version: String,
}
