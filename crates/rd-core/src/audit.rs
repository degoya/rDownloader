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
//! `crates/rd-api/tests/access/audit.rs`.

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
    /// A third-party plugin repository was added, which approves its key (RD-140-01).
    PluginRepositoryAdded,
    /// A plugin repository was switched on or off, or renamed.
    PluginRepositoryChanged,
    /// A third-party plugin repository was removed.
    PluginRepositoryRemoved,
    /// A choice about which plugin version runs (RD-140-02): activated, staged, test ended,
    /// rolled back, tried on one download, or the plugin's update policy set. The `choice`
    /// detail names which.
    PluginVersionChosen,
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
    /// The notifications still queued or retrying were discarded, so they are never sent
    /// (RD-170-11).
    NotificationsDiscarded,
    /// The storage history was emptied; operations still running stayed (RD-180-13).
    StorageHistoryCleared,
    /// The content index was emptied (RD-180-13): duplicate detection by content starts again
    /// from what the next check finds. No file and no download was touched.
    ContentIndexCleared,
    /// A script subscription was created or changed (RD-150-08): which script runs on this
    /// machine, when, and with which arguments. The `change`, `script`, `arguments` and
    /// `schedule` details say what it is now.
    ScriptSubscriptionChanged,
    /// A finished file replaced an existing one because the effective collision policy, or a
    /// person answering a prompt, said `overwrite` (RD-150-01).
    FileOverwritten,
    /// Somebody answered a collision prompt; the `decision` detail names the answer.
    CollisionDecided,
    /// A duplicate file was replaced by a link to its verified identical original (RD-150-02).
    DuplicateLinked,
    /// The full backup's schedule or destination was changed (RD-160-01); the `fields` detail
    /// names what changed, never a path's contents.
    BackupConfigured,
    /// The full backup's passphrase was set or replaced (RD-160-01). The detail carries the new
    /// key's fingerprint, never the passphrase or the key.
    BackupKeyChanged,
    /// A full backup ran (RD-160-01), started by hand or by the schedule; a failed run is
    /// recorded as a failure with its stable code.
    BackupCreated,
    /// An archive was checked at its destination (RD-160-02), by hand or by the verification
    /// schedule; a failed check is recorded as a failure with its stable code.
    BackupVerified,
    /// The service was asked to stop over its API (RD-180-02), by the launcher, the updater or
    /// `rdownloader stop`.
    ServiceStopRequested,
    /// The backup before an update was written and checked, or refused the update (RD-180-03);
    /// the details name both versions and, on a failure, the stable code of the step.
    UpdatePrepared,
    /// An administrator started the self-update (RD-180-02): the details name both versions,
    /// the installation kind and how many downloads were running. How it ended is the update
    /// status and the journal in the data directory, since the service that records this stops.
    UpdateInstallStarted,
    /// The first administrator password was set: the installation stopped being open to whoever
    /// reached it first (audit 2026-09-30).
    SetupCompleted,
    /// A second factor or a passkey was added; the `kind` detail names which. Enrolling one is a
    /// new way in, so it is recorded like a sign-in (audit 2026-09-30).
    MfaEnrolled,
    /// A second factor or a passkey was removed, or the authenticator app switched off.
    MfaRemoved,
    /// The malware scan found something in a finished package (RD-190-14), which stopped it;
    /// the `signature`, `file` and `findings` details say what, where and how much.
    MalwareDetected,
    /// An identity at the identity provider was bound to the administrator (RD-190-15): a new
    /// way in, recorded like an enrolment. The target is the issuer, never the subject.
    IdentityLinked,
    /// The bound identity was released, or went with the provider's configuration.
    IdentityUnlinked,
    /// The password sign-in was switched off from a session the provider opened, or back on
    /// from this machine (`rdownloader auth password-login on`); the `enabled` detail says which.
    PasswordLoginChanged,
    /// The administrator password was set anew on the machine the service runs on, without the
    /// current one (`rdownloader auth reset-password`, RD-190-24); the `path` detail says whether
    /// the running service or the database of a stopped one took it. Never the password.
    PasswordResetLocal,
    /// The download history was emptied (RD-1100-04); the queue and the files were left alone.
    HistoryCleared,
    /// A subscription item that was already decided -- dismissed, skipped or queued before -- was
    /// handed to the LinkGrabber again (RD-1150-04); `previous_state` names what it was, and
    /// `duplicate` whether its address was still there and queued anyway.
    SubscriptionItemRequeued,
}

impl AuditAction {
    /// Every action, in declaration order.
    pub const ALL: [Self; 52] = [
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
        Self::PluginRepositoryAdded,
        Self::PluginRepositoryChanged,
        Self::PluginRepositoryRemoved,
        Self::PluginVersionChosen,
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
        Self::NotificationsDiscarded,
        Self::StorageHistoryCleared,
        Self::ContentIndexCleared,
        Self::ScriptSubscriptionChanged,
        Self::FileOverwritten,
        Self::CollisionDecided,
        Self::DuplicateLinked,
        Self::BackupConfigured,
        Self::BackupKeyChanged,
        Self::BackupCreated,
        Self::BackupVerified,
        Self::ServiceStopRequested,
        Self::UpdatePrepared,
        Self::UpdateInstallStarted,
        Self::SetupCompleted,
        Self::MfaEnrolled,
        Self::MfaRemoved,
        Self::MalwareDetected,
        Self::IdentityLinked,
        Self::IdentityUnlinked,
        Self::PasswordLoginChanged,
        Self::PasswordResetLocal,
        Self::HistoryCleared,
        Self::SubscriptionItemRequeued,
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
            Self::PluginRepositoryAdded => "plugin_repository_added",
            Self::PluginRepositoryChanged => "plugin_repository_changed",
            Self::PluginRepositoryRemoved => "plugin_repository_removed",
            Self::PluginVersionChosen => "plugin_version_chosen",
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
            Self::NotificationsDiscarded => "notifications_discarded",
            Self::StorageHistoryCleared => "storage_history_cleared",
            Self::ContentIndexCleared => "content_index_cleared",
            Self::ScriptSubscriptionChanged => "script_subscription_changed",
            Self::FileOverwritten => "file_overwritten",
            Self::CollisionDecided => "collision_decided",
            Self::DuplicateLinked => "duplicate_linked",
            Self::BackupConfigured => "backup_configured",
            Self::BackupKeyChanged => "backup_key_changed",
            Self::BackupCreated => "backup_created",
            Self::BackupVerified => "backup_verified",
            Self::ServiceStopRequested => "service_stop_requested",
            Self::UpdatePrepared => "update_prepared",
            Self::UpdateInstallStarted => "update_install_started",
            Self::SetupCompleted => "setup_completed",
            Self::MfaEnrolled => "mfa_enrolled",
            Self::MfaRemoved => "mfa_removed",
            Self::MalwareDetected => "malware_detected",
            Self::IdentityLinked => "identity_linked",
            Self::IdentityUnlinked => "identity_unlinked",
            Self::PasswordLoginChanged => "password_login_changed",
            Self::PasswordResetLocal => "password_reset_local",
            Self::HistoryCleared => "history_cleared",
            Self::SubscriptionItemRequeued => "subscription_item_requeued",
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
            "notifications_discarded",
            "storage_history_cleared",
            "content_index_cleared",
            "history_cleared",
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
