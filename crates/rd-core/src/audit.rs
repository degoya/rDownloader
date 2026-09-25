//! The audit event vocabulary (RD-110-03).
//!
//! Only the words live here — which actions are audited, who can have taken one, and how it
//! ended — because `rd-db` stores them, `rd-api` writes them and the viewer reads them, and
//! each would otherwise keep its own spelling of the same twenty strings.
//!
//! **A record never carries a secret value.** Not a password, not a token, not a digest that
//! could be replayed, not a signed URL. An actor is a *kind* and an opaque id; a target is a
//! kind and an id, with the name a person gave it, and that name goes through
//! `rd_core::redact_text` on the way in like everything else. The rule has a canary test in
//! `crates/rd-api/tests/audit.rs`.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What happened. A closed set: an audit log whose vocabulary grows silently is one nobody
/// can write a filter against.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    /// A sign-in was accepted.
    LoginSucceeded,
    /// A sign-in was refused: wrong password, wrong code, or locked out.
    LoginFailed,
    /// A session was closed deliberately.
    Logout,
    /// An API token was accepted on a request. Throttled; see `AUDIT_TOKEN_USE_INTERVAL`.
    TokenUsed,
    /// An API or capture token was minted.
    TokenCreated,
    /// A token was revoked.
    TokenRevoked,
    /// A token's scopes were changed.
    TokenRescoped,
    /// The settings document was written.
    SettingsChanged,
    /// The settings document was reset to the built-in defaults.
    SettingsReset,
    /// A plugin component was installed, which is a trust decision: it was signed by a key
    /// this installation trusts and is now allowed to run.
    PluginInstalled,
    /// A plugin version was removed.
    PluginRemoved,
    /// A plugin signing key was revoked.
    PluginKeyRevoked,
    /// A specific plugin artefact digest was revoked.
    PluginDigestRevoked,
    /// A digest revocation was lifted.
    PluginDigestUnrevoked,
    /// A download was deleted from the queue.
    DownloadDeleted,
    /// A package was deleted.
    PackageDeleted,
    /// A category was deleted.
    CategoryDeleted,
    /// A storage root was deleted.
    StorageRootDeleted,
    /// A configuration backup was restored over the running configuration.
    BackupRestored,
    /// The administrator password was replaced by somebody who knew the old one (RD-120-22).
    PasswordChanged,
    /// The service log was emptied from the settings (RD-120-34).
    LogsCleared,
    /// The audit log was emptied. The record of it is the first entry in the empty log:
    /// clearing an audit log is itself an auditable act, and a trace that removed itself
    /// would leave nobody able to say why the log starts where it does.
    AuditCleared,
    /// The transfer statistics were emptied from the settings (RD-120-34).
    StatsCleared,
    /// The notification history was emptied; deliveries still owed an attempt stayed
    /// (RD-130-08).
    NotificationsCleared,
}

impl AuditAction {
    /// Every action, in declaration order.
    pub const ALL: [Self; 24] = [
        Self::LoginSucceeded,
        Self::LoginFailed,
        Self::Logout,
        Self::TokenUsed,
        Self::TokenCreated,
        Self::TokenRevoked,
        Self::TokenRescoped,
        Self::SettingsChanged,
        Self::SettingsReset,
        Self::PluginInstalled,
        Self::PluginRemoved,
        Self::PluginKeyRevoked,
        Self::PluginDigestRevoked,
        Self::PluginDigestUnrevoked,
        Self::DownloadDeleted,
        Self::PackageDeleted,
        Self::CategoryDeleted,
        Self::StorageRootDeleted,
        Self::BackupRestored,
        Self::PasswordChanged,
        Self::LogsCleared,
        Self::AuditCleared,
        Self::StatsCleared,
        Self::NotificationsCleared,
    ];

    /// The stored word, which is also the filter value and the translation key suffix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LoginSucceeded => "login_succeeded",
            Self::LoginFailed => "login_failed",
            Self::Logout => "logout",
            Self::TokenUsed => "token_used",
            Self::TokenCreated => "token_created",
            Self::TokenRevoked => "token_revoked",
            Self::TokenRescoped => "token_rescoped",
            Self::SettingsChanged => "settings_changed",
            Self::SettingsReset => "settings_reset",
            Self::PluginInstalled => "plugin_installed",
            Self::PluginRemoved => "plugin_removed",
            Self::PluginKeyRevoked => "plugin_key_revoked",
            Self::PluginDigestRevoked => "plugin_digest_revoked",
            Self::PluginDigestUnrevoked => "plugin_digest_unrevoked",
            Self::DownloadDeleted => "download_deleted",
            Self::PackageDeleted => "package_deleted",
            Self::CategoryDeleted => "category_deleted",
            Self::StorageRootDeleted => "storage_root_deleted",
            Self::BackupRestored => "backup_restored",
            Self::PasswordChanged => "password_changed",
            Self::LogsCleared => "logs_cleared",
            Self::AuditCleared => "audit_cleared",
            Self::StatsCleared => "stats_cleared",
            Self::NotificationsCleared => "notifications_cleared",
        }
    }

    /// Parses the stored word; an unknown one is `None` rather than a default, so a filter
    /// with a typo refuses instead of quietly matching everything.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .into_iter()
            .find(|action| action.as_str() == value)
    }
}

/// How the action ended. Two values: an audit log that records intent without outcome cannot
/// answer the only question anybody asks of it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Success,
    Failure,
}

impl AuditOutcome {
    pub const ALL: [Self; 2] = [Self::Success, Self::Failure];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == value)
    }
}

/// Who acted, by kind. The id beside it is opaque and never a credential.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditActorKind {
    /// An interactive session in a browser.
    Session,
    /// A machine credential: an API or capture token.
    Token,
    /// No credential was presented, or none was accepted. A failed sign-in is this.
    Anonymous,
    /// The service itself, acting without anybody asking — a retention sweep, a scheduled
    /// action, a restore run by the binary rather than by a request.
    System,
}

impl AuditActorKind {
    pub const ALL: [Self; 4] = [Self::Session, Self::Token, Self::Anonymous, Self::System];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Token => "token",
            Self::Anonymous => "anonymous",
            Self::System => "system",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

/// How much of the audit log is kept.
///
/// A slice of the `service.settings` blob, so the fields are named exactly as they appear in
/// the settings document. Kept far longer than the diagnostic log by default: the question an
/// audit log answers ("when was this token created, and by whom") is usually asked months
/// later, and the records are small and rare.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct AuditRetentionSettings {
    /// Records kept at most; the oldest go first.
    pub audit_retention_records: u32,
    /// Days a record is kept at most, whatever the count.
    pub audit_retention_days: u32,
}

impl Default for AuditRetentionSettings {
    fn default() -> Self {
        Self {
            audit_retention_records: DEFAULT_AUDIT_RETENTION_RECORDS,
            audit_retention_days: DEFAULT_AUDIT_RETENTION_DAYS,
        }
    }
}

pub const DEFAULT_AUDIT_RETENTION_RECORDS: u32 = 100_000;
pub const DEFAULT_AUDIT_RETENTION_DAYS: u32 = 365;
/// The floor keeps the log worth reading; the ceiling keeps the file and every sweep bounded.
pub const AUDIT_RETENTION_RECORDS_RANGE: RangeInclusive<u32> = 10_000..=2_000_000;
pub const AUDIT_RETENTION_DAYS_RANGE: RangeInclusive<u32> = 30..=3650;

/// How rarely one token's use is recorded.
///
/// A scrape target hits the service every fifteen seconds forever. Recording each of those as
/// an audit event would bury every other record within a day and turn retention into a
/// rolling window of one client's polling. One record per token per interval is what "the
/// token was in use" actually needs to say.
pub const AUDIT_TOKEN_USE_INTERVAL_SECONDS: i64 = 3600;

#[cfg(test)]
mod tests {
    use super::{AuditAction, AuditActorKind, AuditOutcome, AuditRetentionSettings};

    #[test]
    fn every_action_has_a_distinct_word_that_parses_back() {
        let mut seen = std::collections::BTreeSet::new();
        for action in AuditAction::ALL {
            assert!(seen.insert(action.as_str()), "duplicate {action:?}");
            assert_eq!(AuditAction::parse(action.as_str()), Some(action));
        }
        assert_eq!(seen.len(), AuditAction::ALL.len());
        assert_eq!(AuditAction::parse("nothing_like_it"), None);
    }

    #[test]
    fn outcomes_and_actor_kinds_parse_their_own_word_and_nothing_else() {
        for outcome in AuditOutcome::ALL {
            assert_eq!(AuditOutcome::parse(outcome.as_str()), Some(outcome));
        }
        for kind in AuditActorKind::ALL {
            assert_eq!(AuditActorKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(AuditOutcome::parse("partial"), None);
        assert_eq!(AuditActorKind::parse("root"), None);
    }

    #[test]
    fn the_named_actions_of_the_job_all_exist() {
        for word in [
            "login_succeeded",
            "login_failed",
            "token_used",
            "token_created",
            "token_revoked",
            "settings_changed",
            "plugin_installed",
            "plugin_key_revoked",
            "download_deleted",
            "package_deleted",
            "category_deleted",
            "storage_root_deleted",
            "backup_restored",
            "logs_cleared",
            "audit_cleared",
            "stats_cleared",
            "notifications_cleared",
        ] {
            assert!(AuditAction::parse(word).is_some(), "missing {word}");
        }
    }

    #[test]
    fn a_missing_retention_field_reads_as_the_default() {
        let parsed: AuditRetentionSettings =
            serde_json::from_value(serde_json::json!({ "audit_retention_days": 90 }))
                .expect("slice");
        assert_eq!(parsed.audit_retention_days, 90);
        assert_eq!(parsed.audit_retention_records, 100_000);
    }
}
