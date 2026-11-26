//! Operational notices (RD-190-19): what a background check or run has to tell somebody.
//!
//! The hub's other events arrive on the bus, once each. These come from places that repeat --
//! the update check, the plugin repository refresh, an account check -- or that run with nobody
//! watching: a scheduled backup, a token renewal. Each notice carries a key naming what it is
//! about (a version, an account and its end date, a backup run), and `rd_db` queues it to every
//! matching rule at most once per key, in one transaction with the record that it did. So the
//! twentieth check that finds the same version notifies nobody, after a restart too.
//!
//! Like the bus events, the text is in the language the settings name for notifications, English
//! while none is (RD-1480-11): a webhook, a mail box or a chat does not know the reader's. The
//! event code and the body's fields are what a receiver filters on.

use jiff::civil::Date;
use rd_notify::{
    NotificationEvent, NotificationLanguage, Placeholder as P, Subject, Text, Values, Words,
};

/// How close a premium end is before an account check announces it.
pub(crate) const EXPIRY_WARNING_DAYS: i64 = 7;

/// The account-label code a plugin states the premium end under (`plugin_common::label`).
const PREMIUM_UNTIL: &str = "plugin.account.premium_until";
/// The account-label code a plugin states an ended premium under.
const PREMIUM_EXPIRED: &str = "plugin.account.premium_expired";

/// One notice: its event, the key a rule gets it once under, and its text, in no language yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub event: NotificationEvent,
    pub key: String,
    pub title: Text,
    pub body: Text,
    /// What a rule's own wording may name beyond the event and the moment (RD-1280-02): the
    /// version, the plugin, the account, the destination, the error.
    pub values: Values,
}

impl Notice {
    /// A scheduled backup ended without an archive anywhere.
    #[must_use]
    pub fn backup_failed(run_id: &str, code: &str, detail: &str) -> Self {
        Self {
            event: NotificationEvent::BackupFailed,
            key: format!("backup_failed:{run_id}"),
            title: Text::new("backup_failed.title"),
            body: Text::new("backup_failed.body")
                .with("code", code)
                .with("detail", detail),
            values: Values::default().with(P::Error, rd_core::redact_text(detail)),
        }
    }

    /// A scheduled backup reached some destinations and not the others.
    #[must_use]
    pub fn backup_partial(run_id: &str, failed: &[String]) -> Self {
        Self {
            event: NotificationEvent::BackupFailed,
            key: format!("backup_failed:{run_id}"),
            title: Text::new("backup_partial.title"),
            body: Text::new("backup_partial.body").with("failed", failed.join("; ")),
            values: Values::default().with(P::Error, rd_core::redact_text(&failed.join("; "))),
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
            title: Text::new("backup_verify_failed.title"),
            body: Text::new("backup_verify_failed.body")
                .with("archive", archive)
                .with("destination", destination)
                .with("code", code)
                .with("detail", detail),
            values: Values::default()
                .with(P::StorageTarget, destination)
                .with(P::Error, rd_core::redact_text(detail)),
        }
    }

    /// A newer rDownloader is offered.
    #[must_use]
    pub fn update_available(version: &str, current: &str) -> Self {
        Self {
            event: NotificationEvent::UpdateAvailable,
            key: format!("update_available:{version}"),
            title: Text::new("update_available.title").with("version", version),
            body: Text::new("update_available.body")
                .with("version", version)
                .with("current", current),
            values: Values::default().with(P::Version, version),
        }
    }

    /// The automatic install of `version` begins (RD-1240-27): the service stops and comes back
    /// as the new version. Under `update_available`, the event a rule about new versions names.
    #[must_use]
    pub fn update_installing(version: &str, current: &str) -> Self {
        Self {
            event: NotificationEvent::UpdateAvailable,
            key: format!("update_installing:{version}"),
            title: Text::new("update_installing.title").with("version", version),
            body: Text::new("update_installing.body")
                .with("version", version)
                .with("current", current),
            values: Values::default().with(P::Version, version),
        }
    }

    /// The running version came from an update (RD-1240-27).
    #[must_use]
    pub fn update_installed(version: &str, from: &str) -> Self {
        Self {
            event: NotificationEvent::UpdateInstalled,
            key: format!("update_installed:{version}"),
            title: Text::new("update_installed.title").with("version", version),
            body: Text::new("update_installed.body")
                .with("from", from)
                .with("version", version),
            values: Values::default().with(P::Version, version),
        }
    }

    /// An update of rDownloader to `version` did not go ahead, or was taken back (RD-1240-27).
    #[must_use]
    pub fn update_failed(version: &str, current: &str, code: &str) -> Self {
        Self {
            event: NotificationEvent::UpdateFailed,
            key: format!("update_failed:{version}"),
            title: Text::new("update_failed.title").with("version", version),
            body: Text::new("update_failed.body")
                .with("version", version)
                .with("code", code)
                .with("current", current),
            values: Values::default()
                .with(P::Version, version)
                .with(P::Error, code),
        }
    }

    /// The service restarts to apply what waits for the next start (RD-1240-32): `why` names it
    /// in words (nothing: what waits), `automatic` says the service decided it. Keyed by the moment the restart began,
    /// so each restart is announced once.
    #[must_use]
    pub fn service_restarting(started: &str, why: impl Into<Words>, automatic: bool) -> Self {
        let body = if automatic {
            Text::new("service_restarting.body_automatic")
        } else {
            Text::new("service_restarting.body_requested")
        };
        Self {
            event: NotificationEvent::ServiceRestarting,
            key: format!("service_restarting:{started}"),
            title: Text::new("service_restarting.title"),
            body: body.with("why", why),
            values: Values::default(),
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
            title: Text::new("plugin_update_available.title")
                .with("name", name)
                .with("version", version),
            body: Text::new("plugin_update_available.body")
                .with("name", name)
                .with("version", version)
                .with("installed", installed),
            values: Values::default()
                .with(P::Plugin, name)
                .with(P::Version, version),
        }
    }

    /// An automatic plugin update did not install. Keyed like the update itself, so a refresh
    /// that fails at the same version again says nothing new.
    #[must_use]
    pub fn plugin_update_failed(plugin_id: &str, name: &str, version: &str, code: &str) -> Self {
        Self {
            event: NotificationEvent::PluginUpdateFailed,
            key: format!("plugin_update_failed:{plugin_id}:{version}"),
            title: Text::new("plugin_update_failed.title")
                .with("name", name)
                .with("version", version),
            body: Text::new("plugin_update_failed.body")
                .with("name", name)
                .with("version", version)
                .with("code", code),
            values: Values::default()
                .with(P::Plugin, name)
                .with(P::Version, version)
                .with(P::Error, code),
        }
    }

    /// The premium of an account ends on `until`, or ended then (`None`: a date the plugin
    /// did not state).
    #[must_use]
    pub fn account_expiring(account: &rd_core::Account, until: Option<Date>, today: Date) -> Self {
        let name = account_name(account);
        let (when, body) = match until {
            Some(until) if until >= today => (
                until.to_string(),
                Text::new("account_expiring.body_ends").with("date", until.to_string()),
            ),
            Some(until) => (
                until.to_string(),
                Text::new("account_expiring.body_ended_on").with("date", until.to_string()),
            ),
            None => ("ended".to_owned(), Text::new("account_expiring.body_ended")),
        };
        Self {
            event: NotificationEvent::AccountExpiring,
            key: format!("account_expiring:{}:{when}", account.id),
            title: Text::new("account_expiring.title").with("account", name.as_str()),
            body: body.with("account", name.as_str()),
            values: Values::default().with(P::Account, name.as_str()),
        }
    }

    /// An account no longer signs in. At most one a day per account, whichever way it is noticed.
    /// The reason is the provider's words or the service's own.
    #[must_use]
    pub fn account_invalid(
        account: &rd_core::Account,
        reason: impl Into<Words>,
        today: Date,
    ) -> Self {
        let reason = reason.into();
        // What a rule's own wording is filled with is not translated.
        let error = rd_core::redact_text(&reason.say(NotificationLanguage::En));
        let name = account_name(account);
        Self {
            event: NotificationEvent::AccountInvalid,
            key: format!("account_invalid:{}:{today}", account.id),
            title: Text::new("account_invalid.title").with("account", name.as_str()),
            body: Text::new("account_invalid.body")
                .with("account", name.as_str())
                .with("reason", reason),
            values: Values::default()
                .with(P::Account, name.as_str())
                .with(P::Error, error),
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
    today: Date,
) -> Vec<Notice> {
    if !status.valid {
        return vec![Notice::account_invalid(
            account,
            Text::new("account_invalid.refused"),
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
            && until.duration_since(today).as_hours() / 24 <= EXPIRY_WARNING_DAYS
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
    today: Date,
) -> Option<Notice> {
    matches!(
        failure.category,
        rd_core::FailureKind::AccountInvalid | rd_core::FailureKind::AuthRequired
    )
    .then(|| Notice::account_invalid(account, failure.message.as_str(), today))
}

/// The date a premium end starts with: `2027-01-31`, `2027-01-31 00:00:00` and RFC 3339 all
/// begin with it, which is every shape the plugins state.
fn premium_end(text: &str) -> Option<Date> {
    let date = text.trim().get(..10)?;
    date.parse::<rd_core::Date>()
        .ok()
        .map(rd_core::Date::as_jiff)
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
    let rules: Vec<_> = database
        .list_notification_rules()
        .await?
        .into_iter()
        // A notice belongs to no category, so a rule bound to one never takes it.
        .filter(|rule| rule.matches(notice.event, None))
        .collect();
    // A rule's own wording (RD-1280-02), stamped with the service's clock and zone.
    let values = rules
        .iter()
        .any(rd_notify::NotificationRule::has_template)
        .then(|| {
            let mut values = Values::stamped(notice.event, &jiff::ZonedDateTime::now());
            values.extend(&notice.values);
            values
        });
    let language = crate::notify_service::notification_language(database).await;
    let (title, body) = (notice.title.say(language), notice.body.say(language));
    let subject = Subject::now();
    let deliveries: Vec<_> = rules
        .into_iter()
        .map(|rule| {
            let (title, body) = values.as_ref().map_or_else(
                || (title.clone(), body.clone()),
                |values| rule.wording(&title, &body, values),
            );
            rd_db::NewDelivery {
                rule_id: rule.id,
                target_id: rule.target_id,
                idempotency_key: rd_notify::idempotency_key(rule.id, &notice.key),
                event: notice.event,
                title,
                body,
                package_path: None,
                subject,
            }
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
