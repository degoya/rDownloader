//! Operational notices (RD-190-19): what a background check or run has to tell somebody.
//!
//! The hub's other events arrive on the bus, once each. These come from places that repeat --
//! the update check, the plugin repository refresh, an account check -- or that run with nobody
//! watching: a scheduled backup, a token renewal. Each notice carries a key naming what it is
//! about (a version, an account and its end date, a backup run), and `rd_db` queues it to every
//! matching rule at most once per key, in one transaction with the record that it did. So the
//! twentieth check that finds the same version notifies nobody, after a restart too.
//!
//! Like the bus events, the text is English: the delivery goes to a webhook, a mail box or a
//! chat, none of which knows the reader's language, and the event code is what a receiver
//! translates or filters on.

use chrono::NaiveDate;
use rd_notify::NotificationEvent;

/// How close a premium end is before an account check announces it.
pub const EXPIRY_WARNING_DAYS: i64 = 7;

/// The account-label code a plugin states the premium end under (`plugin_common::label`).
const PREMIUM_UNTIL: &str = "plugin.account.premium_until";
/// The account-label code a plugin states an ended premium under.
const PREMIUM_EXPIRED: &str = "plugin.account.premium_expired";

/// One notice: its event, the key a rule gets it once under, and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub event: NotificationEvent,
    pub key: String,
    pub title: String,
    pub body: String,
}

impl Notice {
    /// A scheduled backup ended without an archive anywhere.
    #[must_use]
    pub fn backup_failed(run_id: &str, code: &str, detail: &str) -> Self {
        Self {
            event: NotificationEvent::BackupFailed,
            key: format!("backup_failed:{run_id}"),
            title: "Scheduled backup failed".to_owned(),
            body: format!("The scheduled backup failed ({code}): {detail}"),
        }
    }

    /// A scheduled backup reached some destinations and not the others.
    #[must_use]
    pub fn backup_partial(run_id: &str, failed: &[String]) -> Self {
        Self {
            event: NotificationEvent::BackupFailed,
            key: format!("backup_failed:{run_id}"),
            title: "Scheduled backup incomplete".to_owned(),
            body: format!(
                "The scheduled backup was written, but not to every destination: {}",
                failed.join("; ")
            ),
        }
    }

    /// A scheduled verification found an archive it could not vouch for.
    #[must_use]
    pub fn backup_verify_failed(
        verification_id: &str,
        archive: &str,
        destination: &str,
        code: &str,
        detail: &str,
    ) -> Self {
        Self {
            event: NotificationEvent::BackupVerifyFailed,
            key: format!("backup_verify_failed:{verification_id}"),
            title: "Backup verification failed".to_owned(),
            body: format!(
                "The scheduled verification of {archive} at {destination} failed ({code}): \
                 {detail}"
            ),
        }
    }

    /// A newer rDownloader is offered.
    #[must_use]
    pub fn update_available(version: &str, current: &str) -> Self {
        Self {
            event: NotificationEvent::UpdateAvailable,
            key: format!("update_available:{version}"),
            title: format!("rDownloader {version} is available"),
            body: format!(
                "rDownloader {version} is available; this installation runs {current}. \
                 Settings -> System -> Updates shows the release notes and how to update."
            ),
        }
    }

    /// The automatic install of `version` begins (RD-1240-27): the service stops and comes back
    /// as the new version. Under `update_available`, the event a rule about new versions names.
    #[must_use]
    pub fn update_installing(version: &str, current: &str) -> Self {
        Self {
            event: NotificationEvent::UpdateAvailable,
            key: format!("update_installing:{version}"),
            title: format!("Installing rDownloader {version}"),
            body: format!(
                "Nothing has run for a while, so rDownloader {version} is being installed \
                 automatically over {current}. The service stops and starts again as the new \
                 version; a version that does not start is taken back."
            ),
        }
    }

    /// The running version came from an update (RD-1240-27).
    #[must_use]
    pub fn update_installed(version: &str, from: &str) -> Self {
        Self {
            event: NotificationEvent::UpdateInstalled,
            key: format!("update_installed:{version}"),
            title: format!("rDownloader {version} is installed"),
            body: format!("rDownloader was updated from {from} to {version} and runs again."),
        }
    }

    /// An update of rDownloader to `version` did not go ahead, or was taken back (RD-1240-27).
    #[must_use]
    pub fn update_failed(version: &str, current: &str, code: &str) -> Self {
        Self {
            event: NotificationEvent::UpdateFailed,
            key: format!("update_failed:{version}"),
            title: format!("Update to rDownloader {version} failed"),
            body: format!(
                "The update to {version} did not go ahead ({code}); rDownloader {current} keeps \
                 running. Settings -> System -> Updates shows what happened."
            ),
        }
    }

    /// The service restarts to apply what waits for the next start (RD-1240-32): `why` names it
    /// in words, `automatic` says the service decided it. Keyed by the moment the restart began,
    /// so each restart is announced once.
    #[must_use]
    pub fn service_restarting(started: &str, why: &str, automatic: bool) -> Self {
        let decided = if automatic {
            "Nothing has run for a while, so rDownloader restarts by itself"
        } else {
            "rDownloader restarts on request"
        };
        Self {
            event: NotificationEvent::ServiceRestarting,
            key: format!("service_restarting:{started}"),
            title: "rDownloader restarts".to_owned(),
            body: format!(
                "{decided} to apply {why}. Running downloads are saved by the stop and continue \
                 once it is back."
            ),
        }
    }

    /// A newer version of an installed plugin waits for a click.
    #[must_use]
    pub fn plugin_update_available(
        plugin_id: &str,
        name: &str,
        installed: &str,
        version: &str,
    ) -> Self {
        Self {
            event: NotificationEvent::PluginUpdateAvailable,
            key: format!("plugin_update_available:{plugin_id}:{version}"),
            title: format!("Plugin update: {name} {version}"),
            body: format!(
                "{name} {version} is available; {installed} is installed. Plugins -> Updates \
                 installs it."
            ),
        }
    }

    /// An automatic plugin update did not install. Keyed like the update itself, so a refresh
    /// that fails at the same version again says nothing new.
    #[must_use]
    pub fn plugin_update_failed(plugin_id: &str, name: &str, version: &str, code: &str) -> Self {
        Self {
            event: NotificationEvent::PluginUpdateFailed,
            key: format!("plugin_update_failed:{plugin_id}:{version}"),
            title: format!("Plugin update failed: {name} {version}"),
            body: format!(
                "The automatic update of {name} to {version} was not installed ({code}). The \
                 installed version keeps running; Plugins -> Updates offers it again."
            ),
        }
    }

    /// The premium of an account ends on `until`, or ended then (`None`: a date the plugin
    /// did not state).
    #[must_use]
    pub fn account_expiring(
        account: &rd_core::Account,
        until: Option<NaiveDate>,
        today: NaiveDate,
    ) -> Self {
        let name = account_name(account);
        let (when, body) = match until {
            Some(until) if until >= today => (
                until.to_string(),
                format!("The premium of {name} ends on {until}."),
            ),
            Some(until) => (
                until.to_string(),
                format!("The premium of {name} ended on {until}."),
            ),
            None => (
                "ended".to_owned(),
                format!("The premium of {name} has ended."),
            ),
        };
        Self {
            event: NotificationEvent::AccountExpiring,
            key: format!("account_expiring:{}:{when}", account.id),
            title: format!("Account expiring: {name}"),
            body,
        }
    }

    /// An account no longer signs in. At most one a day per account, whichever way it is noticed.
    #[must_use]
    pub fn account_invalid(account: &rd_core::Account, reason: &str, today: NaiveDate) -> Self {
        let name = account_name(account);
        Self {
            event: NotificationEvent::AccountInvalid,
            key: format!("account_invalid:{}:{today}", account.id),
            title: format!("Account needs attention: {name}"),
            body: format!("{name} could not sign in: {reason}"),
        }
    }
}

/// How the account is named in a notice: its label and its provider, never its user name.
fn account_name(account: &rd_core::Account) -> String {
    format!("{} ({})", account.label, account.provider)
}

/// What a completed account check has to announce.
///
/// An account the provider calls invalid is [`Notice::account_invalid`]. A premium end the
/// label states (`plugin.account.premium_until`, a date first) within
/// [`EXPIRY_WARNING_DAYS`], or a premium the label calls ended, is
/// [`Notice::account_expiring`]. A date in another shape announces nothing rather than a guess.
#[must_use]
pub fn account_check_notices(
    account: &rd_core::Account,
    status: &rd_plugin_host::AccountStatus,
    today: NaiveDate,
) -> Vec<Notice> {
    if !status.valid {
        return vec![Notice::account_invalid(
            account,
            "the provider refused the account",
            today,
        )];
    }
    let mut notices = Vec::new();
    for part in &status.label {
        if part.code == PREMIUM_EXPIRED {
            notices.push(Notice::account_expiring(account, None, today));
        } else if part.code == PREMIUM_UNTIL
            && let Some(until) = part
                .params
                .get("until")
                .map(String::as_str)
                .and_then(premium_end)
            && (until - today).num_days() <= EXPIRY_WARNING_DAYS
        {
            notices.push(Notice::account_expiring(account, Some(until), today));
        }
    }
    notices
}

/// The notice for an account check that failed, when the failure is about the account.
#[must_use]
pub fn account_failure_notice(
    account: &rd_core::Account,
    failure: &rd_core::Failure,
    today: NaiveDate,
) -> Option<Notice> {
    matches!(
        failure.category,
        rd_core::FailureKind::AccountInvalid | rd_core::FailureKind::AuthRequired
    )
    .then(|| Notice::account_invalid(account, &failure.message, today))
}

/// The date a premium end starts with: `2027-01-31`, `2027-01-31 00:00:00` and RFC 3339 all
/// begin with it, which is every shape the plugins state.
fn premium_end(text: &str) -> Option<NaiveDate> {
    let date = text.trim().get(..10)?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

/// Queues `notice` to every rule that wants it and has not had it yet.
///
/// Never fails the caller: a backup, a check or a renewal that could not announce itself has
/// still done its work, so a store failure is logged and that is all.
pub async fn announce(database: &rd_db::Database, notice: Notice) {
    if let Err(error) = queue(database, &notice).await {
        tracing::warn!(%error, event = ?notice.event, "an operational notification could not be queued");
    }
}

async fn queue(database: &rd_db::Database, notice: &Notice) -> anyhow::Result<u64> {
    let deliveries: Vec<_> = database
        .list_notification_rules()
        .await?
        .into_iter()
        // A notice belongs to no category, so a rule bound to one never takes it.
        .filter(|rule| rule.matches(notice.event, None))
        .map(|rule| rd_db::NewDelivery {
            rule_id: rule.id,
            target_id: rule.target_id,
            idempotency_key: rd_notify::idempotency_key(rule.id, &notice.key),
            event: notice.event,
            title: notice.title.clone(),
            body: notice.body.clone(),
        })
        .collect();
    if deliveries.is_empty() {
        return Ok(0);
    }
    database.queue_notification_notice(deliveries).await
}

#[cfg(test)]
#[path = "notify_notice_tests.rs"]
mod tests;
