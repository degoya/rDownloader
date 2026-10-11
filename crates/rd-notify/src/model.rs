//! What can be notified, where it goes and how a delivery is tracked.

use chrono::{DateTime, Utc};
use rd_core::{CategoryId, NotificationRuleId, NotificationTargetId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Where a notification is delivered.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// Signed JSON over HTTPS.
    Webhook,
    /// Mail through a configured SMTP server.
    Smtp,
    /// The external `apprise` CLI, which covers Telegram, Discord, Slack, Matrix, ntfy,
    /// Gotify, Pushover, Home Assistant and about a hundred others.
    Apprise,
    /// A signed notification-destination plugin, named by `config.plugin_id`.
    ///
    /// Delivery for this kind does not live in this crate. A plugin runs in the host's
    /// sandbox, which `rd-notify` deliberately knows nothing about — keeping the transports
    /// testable without a WebAssembly runtime is why the split exists — so the service
    /// dispatches it before calling [`crate::send`].
    Plugin,
    /// Web Push to every browser that turned it on under Settings > Interface (RD-1240-13).
    /// Like a plugin, the service dispatches it: the subscriptions and the signing key live in
    /// its database and vault, and [`crate::send_push`] sends to one browser at a time.
    WebPush,
}

/// How serious an event is; a rule can require a minimum.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// What happened. Deliberately a closed set: a rule filters on it, so it has to be stable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEvent {
    PackageCompleted,
    PackageFailed,
    /// A storage root fell below its free-space threshold (RD-050-15).
    StorageBlocked,
    /// The active profile's traffic budget is used up (RD-050-12).
    BudgetExhausted,
    /// A captcha is waiting for a person.
    CaptchaWaiting,
    /// A queue completion action is counting down (RD-050-13).
    PowerPending,
    /// A scheduled full backup failed, or reached only some of its destinations (RD-190-19).
    BackupFailed,
    /// A scheduled verification found an archive at its destination unreadable, changed or
    /// gone (RD-190-19).
    BackupVerifyFailed,
    /// A newer rDownloader is offered on the update channel (RD-190-19). Once per version.
    UpdateAvailable,
    /// A newer version of an installed plugin waits to be installed by hand (RD-190-19). Once
    /// per plugin and version.
    PluginUpdateAvailable,
    /// An automatic plugin update could not be downloaded or installed (RD-190-19). Once per
    /// plugin and version.
    PluginUpdateFailed,
    /// rDownloader runs a newer version after an update (RD-1240-27). Once per version.
    UpdateInstalled,
    /// An update of rDownloader did not go ahead or was taken back, and the version before it
    /// runs on (RD-1240-27). Once per version.
    UpdateFailed,
    /// rDownloader restarts to apply what waits for the next start, a plugin installed or
    /// updated (RD-1240-32). Once per restart, announced before the stop.
    ServiceRestarting,
    /// An account check reported a premium end within the next days (RD-190-19). Once per
    /// account and end date.
    AccountExpiring,
    /// An account no longer signs in: a check refused it, or a token renewal failed for good
    /// (RD-190-19). At most once per account and day.
    AccountInvalid,
    /// A Usenet download was given up as beyond repair: more PAR2 blocks are missing than its
    /// recovery volumes can replace (RD-1100-02).
    UsenetJobHopeless,
    /// A Usenet server used up its traffic quota (RD-1100-05). Once per crossing of the limit.
    UsenetQuotaReached,
    /// The queue's stop mark was reached and the queue paused after it (RD-1210-02).
    StopMarkReached,
    /// A download left the queue and began to transfer (RD-1240-17). Opt-in, and a burst of
    /// starts is one notification ([`crate::Coalescer`]).
    DownloadStarted,
    /// Links arrived in the LinkGrabber (RD-1240-17). Opt-in, and a burst of imports is one
    /// notification ([`crate::Coalescer`]).
    LinksAdded,
    /// A livestream recording finished and its file is complete (RD-1240-17).
    StreamRecorded,
    /// A subscription poll accepted new items (RD-1240-17). Once per poll that found any.
    SubscriptionMatched,
    /// What an automation's webhook or notify action sends (RD-1240-28). It goes to the one
    /// target the action names, never through a rule, so no rule and no push choice lists it
    /// ([`Self::all`] leaves it out); it had gone out labelled `package_completed`.
    Automation,
}

impl NotificationEvent {
    #[must_use]
    pub fn severity(self) -> Severity {
        match self {
            Self::PackageCompleted
            | Self::UpdateAvailable
            | Self::UpdateInstalled
            | Self::ServiceRestarting
            | Self::PluginUpdateAvailable
            | Self::StopMarkReached
            | Self::DownloadStarted
            | Self::LinksAdded
            | Self::StreamRecorded
            | Self::SubscriptionMatched
            | Self::Automation => Severity::Info,
            Self::PackageFailed
            | Self::BackupFailed
            | Self::BackupVerifyFailed
            | Self::PluginUpdateFailed
            | Self::UpdateFailed
            | Self::AccountInvalid
            | Self::UsenetJobHopeless => Severity::Error,
            Self::StorageBlocked
            | Self::BudgetExhausted
            | Self::CaptchaWaiting
            | Self::PowerPending
            | Self::AccountExpiring
            | Self::UsenetQuotaReached => Severity::Warning,
        }
    }

    /// Every event a rule can name, for the rule editor; [`Self::Automation`] is no rule's.
    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::PackageCompleted,
            Self::PackageFailed,
            Self::StorageBlocked,
            Self::BudgetExhausted,
            Self::CaptchaWaiting,
            Self::PowerPending,
            Self::BackupFailed,
            Self::BackupVerifyFailed,
            Self::UpdateAvailable,
            Self::PluginUpdateAvailable,
            Self::PluginUpdateFailed,
            Self::UpdateInstalled,
            Self::UpdateFailed,
            Self::ServiceRestarting,
            Self::AccountExpiring,
            Self::AccountInvalid,
            Self::UsenetJobHopeless,
            Self::UsenetQuotaReached,
            Self::StopMarkReached,
            Self::DownloadStarted,
            Self::LinksAdded,
            Self::StreamRecorded,
            Self::SubscriptionMatched,
        ]
    }

    /// Whether a rule must name this event to get it (RD-1240-17).
    ///
    /// A rule without an event list means every event, and it meant the outcomes when it was
    /// written: a message for every download that starts and every link that arrives would
    /// turn such a rule into noise. These two reach only a rule that lists them.
    #[must_use]
    pub fn is_opt_in(self) -> bool {
        matches!(self, Self::DownloadStarted | Self::LinksAdded)
    }

    /// Whether a burst of this event is folded into one notification (RD-1240-17).
    #[must_use]
    pub fn coalesces(self) -> bool {
        matches!(self, Self::DownloadStarted | Self::LinksAdded)
    }
}

/// A configured delivery destination. Credentials never live here — only a vault reference.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NotificationTarget {
    pub id: NotificationTargetId,
    pub name: String,
    pub kind: TargetKind,
    pub enabled: bool,
    /// Webhook: the URL. SMTP: `host:port`. Apprise: the service scheme only, because the
    /// full target URL carries the token and stays in the vault.
    pub endpoint: String,
    /// Kind-specific settings without any secret: SMTP TLS mode, sender and recipients,
    /// webhook headers.
    pub config: serde_json::Value,
    /// Vault reference of the webhook signing secret, the SMTP password or the full
    /// apprise target URL. Never returned by the API.
    #[serde(skip_serializing)]
    pub secret_ref: Option<String>,
    /// Whether a secret is stored, so the UI can show that without seeing it.
    pub has_secret: bool,
}

/// Which events of which packages reach which target.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NotificationRule {
    pub id: NotificationRuleId,
    pub name: String,
    pub enabled: bool,
    pub target_id: NotificationTargetId,
    /// Empty means every event except the opt-in ones ([`NotificationEvent::is_opt_in`]).
    pub events: Vec<NotificationEvent>,
    /// Restricts the rule to one category; `None` matches all.
    pub category_id: Option<CategoryId>,
    pub min_severity: Severity,
}

impl NotificationRule {
    /// Whether this rule wants an event of `severity` in `category`.
    #[must_use]
    pub fn matches(&self, event: NotificationEvent, category_id: Option<CategoryId>) -> bool {
        if !self.enabled {
            return false;
        }
        let listed = if self.events.is_empty() {
            !event.is_opt_in()
        } else {
            self.events.contains(&event)
        };
        if !listed {
            return false;
        }
        if event.severity() < self.min_severity {
            return false;
        }
        match (self.category_id, category_id) {
            // A rule bound to a category ignores events that belong to none.
            (Some(wanted), Some(actual)) => wanted == actual,
            (Some(_), None) => false,
            (None, _) => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Queued,
    Delivered,
    /// Retryable failure; `next_attempt_at` says when the worker tries again.
    Retrying,
    /// Gave up after the last attempt.
    Failed,
}

/// One attempt to deliver one event to one target.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Delivery {
    pub id: rd_core::NotificationDeliveryId,
    pub rule_id: NotificationRuleId,
    pub target_id: NotificationTargetId,
    /// Derived from the rule and the event, and unique — so one event produces at most one
    /// delivery per matching rule even if the worker restarts mid-flight.
    pub idempotency_key: String,
    pub event: NotificationEvent,
    pub title: String,
    pub body: String,
    pub state: DeliveryState,
    pub attempt: u32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub response_status: Option<u16>,
    /// Short, redacted excerpt of the response, for support.
    pub response_excerpt: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use rd_core::{CategoryId, NotificationRuleId, NotificationTargetId};

    use super::{NotificationEvent, NotificationRule, Severity};

    fn rule() -> NotificationRule {
        NotificationRule {
            id: NotificationRuleId::new(),
            name: "test".to_owned(),
            enabled: true,
            target_id: NotificationTargetId::new(),
            events: Vec::new(),
            category_id: None,
            min_severity: Severity::Info,
        }
    }

    #[test]
    fn an_empty_event_list_matches_everything() {
        let rule = rule();
        assert!(rule.matches(NotificationEvent::PackageCompleted, None));
        assert!(rule.matches(NotificationEvent::PackageFailed, None));
    }

    #[test]
    fn a_minimum_severity_filters_the_quieter_events_out() {
        let mut rule = rule();
        rule.min_severity = Severity::Error;
        assert!(!rule.matches(NotificationEvent::PackageCompleted, None));
        assert!(rule.matches(NotificationEvent::PackageFailed, None));
    }

    #[test]
    fn a_category_rule_ignores_events_of_other_categories_and_of_none() {
        let mut rule = rule();
        let wanted = CategoryId::new();
        rule.category_id = Some(wanted);
        assert!(rule.matches(NotificationEvent::PackageCompleted, Some(wanted)));
        assert!(!rule.matches(NotificationEvent::PackageCompleted, Some(CategoryId::new())));
        assert!(!rule.matches(NotificationEvent::PackageCompleted, None));
    }

    #[test]
    fn the_operational_events_have_their_severity_and_their_wire_name() {
        let cases = [
            (
                NotificationEvent::BackupFailed,
                Severity::Error,
                "backup_failed",
            ),
            (
                NotificationEvent::BackupVerifyFailed,
                Severity::Error,
                "backup_verify_failed",
            ),
            (
                NotificationEvent::PluginUpdateFailed,
                Severity::Error,
                "plugin_update_failed",
            ),
            (
                NotificationEvent::UpdateAvailable,
                Severity::Info,
                "update_available",
            ),
            (
                NotificationEvent::PluginUpdateAvailable,
                Severity::Info,
                "plugin_update_available",
            ),
            (
                NotificationEvent::UpdateInstalled,
                Severity::Info,
                "update_installed",
            ),
            (
                NotificationEvent::UpdateFailed,
                Severity::Error,
                "update_failed",
            ),
            (
                NotificationEvent::ServiceRestarting,
                Severity::Info,
                "service_restarting",
            ),
            (
                NotificationEvent::AccountExpiring,
                Severity::Warning,
                "account_expiring",
            ),
            (
                NotificationEvent::AccountInvalid,
                Severity::Error,
                "account_invalid",
            ),
            (
                NotificationEvent::UsenetJobHopeless,
                Severity::Error,
                "usenet_job_hopeless",
            ),
            (
                NotificationEvent::UsenetQuotaReached,
                Severity::Warning,
                "usenet_quota_reached",
            ),
            (
                NotificationEvent::StopMarkReached,
                Severity::Info,
                "stop_mark_reached",
            ),
            (
                NotificationEvent::DownloadStarted,
                Severity::Info,
                "download_started",
            ),
            (NotificationEvent::LinksAdded, Severity::Info, "links_added"),
            (
                NotificationEvent::StreamRecorded,
                Severity::Info,
                "stream_recorded",
            ),
            (
                NotificationEvent::SubscriptionMatched,
                Severity::Info,
                "subscription_matched",
            ),
        ];
        for (event, severity, name) in cases {
            assert_eq!(event.severity(), severity, "{name}");
            assert_eq!(
                serde_json::to_value(event).expect("serialize"),
                serde_json::json!(name)
            );
            assert!(NotificationEvent::all().contains(&event), "{name}");
        }
    }

    /// RD-1240-28: an automation's message has a label of its own, and no rule names it.
    #[test]
    fn an_automation_message_is_its_own_event_and_no_rule_s() {
        let event = NotificationEvent::Automation;
        assert_eq!(
            serde_json::to_value(event).expect("serialize"),
            serde_json::json!("automation")
        );
        assert_eq!(event.severity(), Severity::Info);
        assert!(!NotificationEvent::all().contains(&event));
    }

    #[test]
    fn the_opt_in_events_reach_only_a_rule_that_lists_them() {
        let mut rule = rule();
        assert!(!rule.matches(NotificationEvent::DownloadStarted, None));
        assert!(!rule.matches(NotificationEvent::LinksAdded, None));
        // The other new events are outcomes, which a rule for every event takes.
        assert!(rule.matches(NotificationEvent::StreamRecorded, None));
        assert!(rule.matches(NotificationEvent::SubscriptionMatched, None));
        rule.events = vec![
            NotificationEvent::DownloadStarted,
            NotificationEvent::LinksAdded,
        ];
        assert!(rule.matches(NotificationEvent::DownloadStarted, None));
        assert!(rule.matches(NotificationEvent::LinksAdded, None));
        assert!(!rule.matches(NotificationEvent::StreamRecorded, None));
    }

    #[test]
    fn a_disabled_rule_matches_nothing() {
        let mut rule = rule();
        rule.enabled = false;
        assert!(!rule.matches(NotificationEvent::PackageFailed, None));
    }
}
