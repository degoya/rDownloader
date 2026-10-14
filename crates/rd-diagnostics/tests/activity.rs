//! `notifications.json` and `queue.json` (RD-1291-04): bounded, stating their bounds, redacted
//! like every other entry and named in the manifest.

use std::io::Read;

use chrono::{Duration, TimeZone, Utc};
use rd_db::{
    DeliveryCount, DiagnosticDelivery, DiagnosticRule, DiagnosticTarget, DownloadCount,
    NotificationDiagnostics, PackageTransition, QueueDiagnostics, ValueCount,
};
use rd_diagnostics::{
    BundleInput,
    activity::{DELIVERY_WINDOW_DAYS, RECENT_DELIVERIES_LIMIT, RECENT_TRANSITIONS_LIMIT},
    bundle,
};

fn input() -> BundleInput {
    let start = Utc
        .with_ymd_and_hms(2026, 10, 14, 12, 0, 0)
        .single()
        .expect("time");
    // Newest first, as `rd-db` reads them, and more than the bundle keeps.
    let recent_deliveries = (0..60)
        .map(|index| DiagnosticDelivery {
            rule_id: "rule-1".to_owned(),
            event: "package_completed".to_owned(),
            state: if index == 0 { "failed" } else { "delivered" }.to_owned(),
            attempts: 1,
            response_status: Some(if index == 0 { 500 } else { 200 }),
            queued_at: start - Duration::minutes(index),
            updated_at: start - Duration::minutes(index),
        })
        .collect();
    let recent_transitions = (0..60)
        .map(|index| PackageTransition {
            package_id: format!("package-{index}"),
            previous: Some("postprocessing".to_owned()),
            state: "completed".to_owned(),
            stage: None,
            category_id: None,
            occurred_at: start - Duration::minutes(index),
        })
        .collect();
    BundleInput {
        application_version: "1.29.1".to_owned(),
        os: "linux".to_owned(),
        arch: "x86_64".to_owned(),
        notifications: NotificationDiagnostics {
            targets: vec![DiagnosticTarget {
                id: "target-1".to_owned(),
                kind: "plugin".to_owned(),
                enabled: true,
                plugin_id: Some("ntfy".to_owned()),
            }],
            rules: vec![DiagnosticRule {
                id: "rule-1".to_owned(),
                target_id: "target-1".to_owned(),
                enabled: true,
                events: vec!["package_completed".to_owned()],
                category_id: None,
                min_severity: "info".to_owned(),
                has_title_template: true,
                has_body_template: false,
            }],
            delivery_counts: vec![DeliveryCount {
                event: "package_completed".to_owned(),
                state: "delivered".to_owned(),
                count: 59,
            }],
            recent_deliveries,
        },
        queue: QueueDiagnostics {
            packages_by_state: vec![ValueCount {
                value: "completed".to_owned(),
                count: 60,
            }],
            packages_by_stage: Vec::new(),
            recent_transitions,
            downloads: vec![DownloadCount {
                state: "failed".to_owned(),
                error_kind: Some("permanent".to_owned()),
                // Whatever a code carries, it passes the text redaction like every other value.
                error_code: Some("https://cdn.example/f?token=sk-live-4242".to_owned()),
                count: 1,
            }],
        },
        ..BundleInput::default()
    }
}

fn read(bytes: &[u8], name: &str) -> String {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut text = String::new();
    archive
        .by_name(name)
        .expect("entry")
        .read_to_string(&mut text)
        .expect("read");
    text
}

#[test]
fn both_entries_keep_the_newest_rows_within_their_limits_and_say_so() {
    let built = bundle::build(
        &input(),
        &["notifications".to_owned(), "queue".to_owned()],
        Utc::now(),
    )
    .expect("bundle");

    let notifications: serde_json::Value =
        serde_json::from_str(&read(&built.bytes, "notifications.json")).expect("json");
    assert_eq!(notifications["window_days"], DELIVERY_WINDOW_DAYS);
    assert_eq!(notifications["recent_limit"], RECENT_DELIVERIES_LIMIT);
    let deliveries = notifications["recent_deliveries"]
        .as_array()
        .expect("deliveries");
    assert_eq!(deliveries.len(), RECENT_DELIVERIES_LIMIT);
    assert_eq!(deliveries[0]["state"], "failed", "the newest stays first");
    assert_eq!(deliveries[0]["response_status"], 500);
    assert_eq!(notifications["targets"][0]["plugin_id"], "ntfy");
    assert_eq!(notifications["rules"][0]["has_title_template"], true);
    assert_eq!(notifications["delivery_counts"][0]["count"], 59);

    let queue: serde_json::Value =
        serde_json::from_str(&read(&built.bytes, "queue.json")).expect("json");
    assert_eq!(queue["transition_limit"], RECENT_TRANSITIONS_LIMIT);
    let transitions = queue["recent_transitions"].as_array().expect("transitions");
    assert_eq!(transitions.len(), RECENT_TRANSITIONS_LIMIT);
    assert_eq!(transitions[0]["package_id"], "package-0");
    assert_eq!(queue["packages_by_state"][0]["value"], "completed");
    assert_eq!(queue["downloads"][0]["error_kind"], "permanent");
    let code = queue["downloads"][0]["error_code"].as_str().expect("code");
    assert!(!code.contains("sk-live-4242"), "{code}");
}

#[test]
fn the_preview_and_the_manifest_name_both_entries_their_bounds_and_their_redactions() {
    let inventory = bundle::inventory(&input()).expect("inventory");
    let notifications = inventory
        .entries
        .iter()
        .find(|entry| entry.id == "notifications")
        .expect("notifications entry");
    let queue = inventory
        .entries
        .iter()
        .find(|entry| entry.id == "queue")
        .expect("queue entry");
    // The descriptions name the bounds in words; they must name the ones the code applies.
    for (entry, bounds) in [
        (
            notifications,
            [
                DELIVERY_WINDOW_DAYS.to_string(),
                RECENT_DELIVERIES_LIMIT.to_string(),
            ],
        ),
        (
            queue,
            [
                RECENT_TRANSITIONS_LIMIT.to_string(),
                RECENT_TRANSITIONS_LIMIT.to_string(),
            ],
        ),
    ] {
        for bound in bounds {
            assert!(
                entry.description.text.contains(&bound),
                "{}: {}",
                entry.id,
                entry.description.text
            );
        }
        assert_eq!(entry.redactions.len(), 2, "{}", entry.id);
    }
    assert_eq!(notifications.items, 1 + 1 + 1 + 50);
    assert_eq!(queue.items, 1 + 50 + 1);

    let ids: Vec<String> = inventory
        .entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect();
    let built = bundle::build(&input(), &ids, Utc::now()).expect("bundle");
    for path in ["notifications.json", "queue.json"] {
        assert!(
            built
                .manifest
                .entries
                .iter()
                .any(|entry| entry.path == path),
            "{path} in the manifest"
        );
        assert_eq!(
            built
                .manifest
                .redactions
                .iter()
                .filter(|line| line.starts_with(&format!("{path}: ")))
                .count(),
            2,
            "{path}: {:?}",
            built.manifest.redactions
        );
    }
    assert_eq!(
        built.manifest.excluded.len(),
        5,
        "{:?}",
        built.manifest.excluded
    );
    assert_eq!(built.manifest.excluded.len(), inventory.excluded.len());
}
