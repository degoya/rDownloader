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
    /// Push to every browser that turned it on under Settings > Interface; the endpoint is not
    /// used. The first browser that turns push on makes one by itself.
    WebPush,
}

impl From<TargetKindParam> for rd_notify::TargetKind {
    fn from(value: TargetKindParam) -> Self {
        match value {
            TargetKindParam::Webhook => Self::Webhook,
            TargetKindParam::Smtp => Self::Smtp,
            TargetKindParam::Apprise => Self::Apprise,
            TargetKindParam::Plugin => Self::Plugin,
            TargetKindParam::WebPush => Self::WebPush,
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
    /// A scheduled full backup failed or missed a destination.
    BackupFailed,
    /// A scheduled backup verification failed.
    BackupVerifyFailed,
    /// A newer rDownloader is offered; once per version.
    UpdateAvailable,
    /// A newer version of an installed plugin waits to be installed; once per version.
    PluginUpdateAvailable,
    /// An automatic plugin update was not installed; once per plugin and version.
    PluginUpdateFailed,
    /// rDownloader runs a newer version after an update; once per version.
    UpdateInstalled,
    /// An update of rDownloader did not go ahead or was taken back; once per version.
    UpdateFailed,
    /// rDownloader restarts to apply a plugin installed or updated; once per restart.
    ServiceRestarting,
    /// An account check found the premium ending within seven days, or ended.
    AccountExpiring,
    /// An account no longer signs in: a check refused it or a token renewal failed.
    AccountInvalid,
    /// A Usenet download was given up as beyond repair (usenet.job_hopeless).
    UsenetJobHopeless,
    /// A Usenet server used up its traffic quota; once per crossing of the limit.
    UsenetQuotaReached,
    /// The queue's stop mark was reached and the queue paused after it.
    StopMarkReached,
    /// Downloads began to transfer; a burst of starts is one notification. Opt-in: only a
    /// rule that lists it gets it.
    DownloadStarted,
    /// Links arrived in the LinkGrabber; a burst of imports is one notification. Opt-in: only
    /// a rule that lists it gets it.
    LinksAdded,
    /// A livestream recording finished.
    StreamRecorded,
    /// A subscription check accepted new items.
    SubscriptionMatched,
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
            NotificationEventParam::BackupFailed => Self::BackupFailed,
            NotificationEventParam::BackupVerifyFailed => Self::BackupVerifyFailed,
            NotificationEventParam::UpdateAvailable => Self::UpdateAvailable,
            NotificationEventParam::PluginUpdateAvailable => Self::PluginUpdateAvailable,
            NotificationEventParam::PluginUpdateFailed => Self::PluginUpdateFailed,
            NotificationEventParam::UpdateInstalled => Self::UpdateInstalled,
            NotificationEventParam::UpdateFailed => Self::UpdateFailed,
            NotificationEventParam::ServiceRestarting => Self::ServiceRestarting,
            NotificationEventParam::AccountExpiring => Self::AccountExpiring,
            NotificationEventParam::AccountInvalid => Self::AccountInvalid,
            NotificationEventParam::UsenetJobHopeless => Self::UsenetJobHopeless,
            NotificationEventParam::UsenetQuotaReached => Self::UsenetQuotaReached,
            NotificationEventParam::StopMarkReached => Self::StopMarkReached,
            NotificationEventParam::DownloadStarted => Self::DownloadStarted,
            NotificationEventParam::LinksAdded => Self::LinksAdded,
            NotificationEventParam::StreamRecorded => Self::StreamRecorded,
            NotificationEventParam::SubscriptionMatched => Self::SubscriptionMatched,
        }
    }
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateNotificationTargetParams {
    pub name: String,
    pub kind: TargetKindParam,
    /// Webhook: the URL — the person's own network and this machine are fine, a link-local
    /// address and rDownloader's own ports are refused at delivery, and a redirect is not
    /// followed. SMTP: `host:port`. Apprise: the service scheme.
    pub endpoint: String,
    /// Settings without any secret: SMTP TLS mode, sender and recipients, webhook headers,
    /// `plugin_id` for a plugin destination and its `settings` object (name to value, as the
    /// destination list declares them; ntfy's `priority_info`, for example). Apprise:
    /// `executable`, the program it runs, which needs `api:admin`.
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct UpdateNotificationTargetParams {
    pub id: String,
    pub name: Option<String>,
    pub kind: Option<TargetKindParam>,
    pub endpoint: Option<String>,
    /// Replaces the whole configuration. Setting or changing an apprise `executable` needs
    /// `api:admin`.
    pub config: Option<serde_json::Map<String, serde_json::Value>>,
    pub enabled: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct CreateNotificationRuleParams {
    pub name: String,
    /// Destination this rule delivers to.
    pub target_id: String,
    /// Events the rule reacts to; empty means every event except download_started and
    /// links_added, which a rule gets only by listing them. The operational ones (backup,
    /// verification, update, plugin update, account) and links_added belong to no category, so
    /// a rule restricted to one never receives them.
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

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct RemoveSupersededPluginVersionsParams {
    /// Plugin id as `list_configuration(section = "plugins")` reports it; every plugin when left
    /// out.
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ListBundledServicesParams {
    /// Language of the names and descriptions (`de`, `en`, `es`, `fr`); English when left out.
    #[serde(default)]
    pub locale: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct InstallBundledServicesParams {
    /// Service keys as `list_bundled_services` reports them.
    pub services: Vec<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct RemoveBundledServicesParams {
    /// Service keys as `list_bundled_services` reports them.
    pub services: Vec<String>,
}
