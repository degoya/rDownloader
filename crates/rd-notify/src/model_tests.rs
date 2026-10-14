//! Rules, severities and the events' wire names.

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
        title_template: None,
        body_template: None,
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

/// RD-1290-04: the queue's moments carry their wire names and reach only a rule that names them.
#[test]
fn the_queue_s_moments_are_opt_in() {
    let mut rule = rule();
    for (event, name) in [
        (NotificationEvent::QueuePaused, "queue_paused"),
        (NotificationEvent::QueueResumed, "queue_resumed"),
        (NotificationEvent::QueueFinished, "queue_finished"),
    ] {
        assert_eq!(
            serde_json::to_value(event).expect("serialize"),
            serde_json::json!(name)
        );
        assert_eq!(event.severity(), Severity::Info, "{name}");
        assert!(NotificationEvent::all().contains(&event), "{name}");
        assert!(event.takes_template(), "{name}: one moment, no burst");
        assert!(!rule.matches(event, None), "{name}: a rule for every event");
    }
    rule.events = vec![
        NotificationEvent::QueuePaused,
        NotificationEvent::QueueFinished,
    ];
    assert!(rule.matches(NotificationEvent::QueuePaused, None));
    assert!(rule.matches(NotificationEvent::QueueFinished, None));
    assert!(!rule.matches(NotificationEvent::QueueResumed, None));
}
