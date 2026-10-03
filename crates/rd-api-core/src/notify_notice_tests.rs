//! The operational notices' keys and the account check's reading (RD-190-19).

use chrono::NaiveDate;
use rd_notify::NotificationEvent;
use rd_plugin_host::{AccountStatus, LabelPart};

use super::{Notice, account_check_notices, account_failure_notice, premium_end};

fn account() -> rd_core::Account {
    rd_core::Account {
        id: rd_core::AccountId::new(),
        provider: "rapidgator".to_owned(),
        label: "Main".to_owned(),
        username: Some("someone@example.test".to_owned()),
        credential_mode: None,
        proxy_profile_id: None,
        enabled: true,
        has_secret: true,
        has_cookies: false,
    }
}

fn day(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").expect("date")
}

fn status(valid: bool, label: Vec<LabelPart>) -> AccountStatus {
    AccountStatus {
        valid,
        premium: valid,
        label,
        traffic_left: None,
    }
}

fn part(code: &str, until: Option<&str>) -> LabelPart {
    LabelPart {
        code: code.to_owned(),
        params: until
            .map(|until| [("until".to_owned(), until.to_owned())].into())
            .unwrap_or_default(),
        message: String::new(),
    }
}

#[test]
fn a_scheduled_backup_is_announced_once_per_run_whichever_way_it_failed() {
    let failed = Notice::backup_failed("run-1", "backup.destination_unreachable", "gone");
    let partial = Notice::backup_partial("run-1", &["NAS: refused".to_owned()]);
    assert_eq!(failed.event, NotificationEvent::BackupFailed);
    assert_eq!(partial.event, NotificationEvent::BackupFailed);
    assert_eq!(failed.key, "backup_failed:run-1");
    assert_eq!(failed.key, partial.key, "one run, one notice");
    assert!(failed.body.contains("backup.destination_unreachable"));
    assert!(partial.body.contains("NAS: refused"));
}

#[test]
fn a_failed_verification_is_announced_once_per_verification() {
    let notice = Notice::backup_verify_failed(
        "verify-1",
        "rdownloader-2026.rdbackup",
        "NAS",
        "backup.verify_digest_mismatch",
        "changed",
    );
    assert_eq!(notice.event, NotificationEvent::BackupVerifyFailed);
    assert_eq!(notice.key, "backup_verify_failed:verify-1");
    assert!(notice.body.contains("rdownloader-2026.rdbackup"));
    assert!(notice.body.contains("backup.verify_digest_mismatch"));
}

#[test]
fn a_failed_automatic_plugin_update_is_keyed_by_plugin_and_version() {
    let notice =
        Notice::plugin_update_failed("rapidgator", "Rapidgator", "1.3.0", "repository.offline");
    assert_eq!(notice.event, NotificationEvent::PluginUpdateFailed);
    assert_eq!(notice.key, "plugin_update_failed:rapidgator:1.3.0");
    assert_ne!(
        notice.key,
        Notice::plugin_update_available("rapidgator", "Rapidgator", "1.2.0", "1.3.0").key,
        "failing and waiting are two notices"
    );
    assert!(notice.body.contains("repository.offline"));
}

#[test]
fn an_update_is_keyed_by_its_version_and_a_plugin_update_by_plugin_and_version() {
    let update = Notice::update_available("1.9.1", "1.9.0");
    assert_eq!(update.event, NotificationEvent::UpdateAvailable);
    assert_eq!(update.key, "update_available:1.9.1");
    assert_eq!(
        Notice::update_available("1.9.1", "1.8.1").key,
        update.key,
        "the version that checks does not make it a new notice"
    );
    let plugin = Notice::plugin_update_available("rapidgator", "Rapidgator", "1.2.0", "1.3.0");
    assert_eq!(plugin.event, NotificationEvent::PluginUpdateAvailable);
    assert_eq!(plugin.key, "plugin_update_available:rapidgator:1.3.0");
    assert!(plugin.body.contains("1.2.0"));
}

#[test]
fn a_premium_end_within_a_week_is_announced_once_per_date() {
    let account = account();
    let today = day("2026-10-02");
    let soon = account_check_notices(
        &account,
        &status(
            true,
            vec![part(
                "plugin.account.premium_until",
                Some("2026-10-08 23:59:59"),
            )],
        ),
        today,
    );
    assert_eq!(soon.len(), 1, "{soon:?}");
    assert_eq!(soon[0].event, NotificationEvent::AccountExpiring);
    assert_eq!(
        soon[0].key,
        format!("account_expiring:{}:2026-10-08", account.id)
    );
    assert!(soon[0].body.contains("ends on 2026-10-08"), "{soon:?}");
    assert!(
        !soon[0].body.contains("someone@example.test"),
        "a notice never names the user name"
    );

    let later = account_check_notices(
        &account,
        &status(
            true,
            vec![part("plugin.account.premium_until", Some("2026-10-20"))],
        ),
        today,
    );
    assert!(later.is_empty(), "{later:?}");
}

#[test]
fn an_ended_premium_is_announced_and_an_unreadable_date_is_not_guessed() {
    let account = account();
    let today = day("2026-10-02");
    let ended = account_check_notices(
        &account,
        &status(true, vec![part("plugin.account.premium_expired", None)]),
        today,
    );
    assert_eq!(ended.len(), 1);
    assert_eq!(
        ended[0].key,
        format!("account_expiring:{}:ended", account.id)
    );
    let past = account_check_notices(
        &account,
        &status(
            true,
            vec![part("plugin.account.premium_until", Some("2026-09-30"))],
        ),
        today,
    );
    assert!(past[0].body.contains("ended on 2026-09-30"), "{past:?}");

    assert_eq!(premium_end("2027-01-31T00:00:00Z"), Some(day("2027-01-31")));
    assert_eq!(premium_end("31.01.2027"), None);
    assert_eq!(premium_end("soon"), None);
}

#[test]
fn an_invalid_account_is_announced_at_most_once_a_day() {
    let account = account();
    let today = day("2026-10-02");
    let refused = account_check_notices(&account, &status(false, Vec::new()), today);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].event, NotificationEvent::AccountInvalid);
    assert_eq!(
        refused[0].key,
        format!("account_invalid:{}:2026-10-02", account.id)
    );

    let failure = rd_core::Failure::new(rd_core::FailureKind::AccountInvalid, "wrong password");
    let failed = account_failure_notice(&account, &failure, today).expect("a notice");
    assert_eq!(
        failed.key, refused[0].key,
        "one a day, however it was noticed"
    );
    assert!(failed.body.contains("wrong password"));

    let offline = rd_core::Failure::new(
        rd_core::FailureKind::Transient {
            retry_after_seconds: None,
        },
        "timeout",
    );
    assert!(
        account_failure_notice(&account, &offline, today).is_none(),
        "an unreachable provider says nothing about the account"
    );
}
