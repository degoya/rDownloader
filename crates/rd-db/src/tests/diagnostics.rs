//! What the diagnostic bundle reads about notifications and the queue (RD-1291-04): the
//! window, the limits, and that no title, text, folder, endpoint or name is ever selected.

use chrono::{Duration, Utc};
use rd_core::{AuthProfileSelection, DownloadId, PackageId, PackageState, PostprocessStage};
use rd_notify::{DeliveryState, NotificationEvent, Severity, TargetKind};

use super::{routing_category, routing_root, temp_database};
use crate::{Database, NewDownload, NewPackage};

/// Strings the seeded rows carry where a bundle must never show them.
const CANARIES: &[&str] = &[
    "endpoint-canary",
    "topic-canary",
    "header-canary",
    "rule-name-canary",
    "title-template-canary",
    "title-canary",
    "body-canary",
    "path-canary",
    "excerpt-canary",
    "package-name-canary",
    "folder-canary",
    "file-name-canary",
    "url-canary",
    "current-file-canary",
    "message-canary",
];

async fn raw(directory: &std::path::Path) -> sqlx::SqliteConnection {
    <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new().filename(directory.join(super::DATABASE_FILE)),
    )
    .await
    .expect("connect")
}

async fn seed_notifications(database: &Database) -> rd_notify::NotificationRule {
    let webhook = database
        .upsert_notification_target(
            None,
            crate::NewNotificationTarget {
                name: "Webhook".to_owned(),
                kind: TargetKind::Webhook,
                enabled: true,
                endpoint: "https://hooks.example/endpoint-canary".to_owned(),
                config: serde_json::json!({ "headers": { "X-Token": "header-canary" } }),
                secret_ref: None,
                clear_secret: false,
            },
        )
        .await
        .expect("webhook target");
    database
        .upsert_notification_target(
            None,
            crate::NewNotificationTarget {
                name: "Phone".to_owned(),
                kind: TargetKind::Plugin,
                enabled: false,
                endpoint: "ntfy-endpoint-canary".to_owned(),
                config: serde_json::json!({ "plugin_id": "ntfy", "topic": "topic-canary" }),
                secret_ref: None,
                clear_secret: false,
            },
        )
        .await
        .expect("plugin target");
    database
        .upsert_notification_rule(
            None,
            crate::NewNotificationRule {
                name: "rule-name-canary".to_owned(),
                enabled: true,
                target_id: webhook.id,
                events: vec![NotificationEvent::PackageCompleted],
                category_id: None,
                min_severity: Severity::Warning,
                title_template: Some("title-template-canary {package}".to_owned()),
                body_template: None,
            },
        )
        .await
        .expect("rule")
}

#[tokio::test]
async fn notifications_are_read_without_wording_within_their_window_and_limit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = temp_database(directory.path()).await;
    let rule = seed_notifications(&database).await;
    for index in 0..60 {
        database
            .queue_notification_delivery(crate::NewDelivery {
                rule_id: rule.id,
                target_id: rule.target_id,
                idempotency_key: format!("event-{index}"),
                event: NotificationEvent::PackageCompleted,
                title: "title-canary".to_owned(),
                body: "body-canary".to_owned(),
                package_path: Some("/srv/path-canary".to_owned()),
            })
            .await
            .expect("queue delivery");
    }
    for delivery in database
        .list_notification_deliveries(u32::MAX)
        .await
        .expect("deliveries")
    {
        let index: u32 = delivery.idempotency_key["event-".len()..]
            .parse()
            .expect("index");
        let (state, status) = match index {
            10..20 => (DeliveryState::Delivered, 200),
            20..25 => (DeliveryState::Failed, 500),
            _ => continue,
        };
        database
            .record_notification_attempt(
                delivery.id,
                state,
                3,
                None,
                Some(status),
                Some("excerpt-canary".to_owned()),
            )
            .await
            .expect("attempt");
    }
    // Five deliveries from before the window: listed among the newest if they were, never
    // counted.
    let mut connection = raw(directory.path()).await;
    sqlx::query(
        "UPDATE notification_deliveries SET created_at = ? \
         WHERE idempotency_key IN ('event-0', 'event-1', 'event-2', 'event-3', 'event-4')",
    )
    .bind(Utc::now() - Duration::days(8))
    .execute(&mut connection)
    .await
    .expect("backdate");

    let read = database
        .notification_diagnostics(Utc::now() - Duration::days(7), 50)
        .await
        .expect("diagnostics");

    let counts: Vec<(&str, &str, u64)> = read
        .delivery_counts
        .iter()
        .map(|row| (row.event.as_str(), row.state.as_str(), row.count))
        .collect();
    assert_eq!(
        counts,
        [
            ("package_completed", "delivered", 10),
            ("package_completed", "failed", 5),
            ("package_completed", "queued", 40),
        ]
    );
    assert_eq!(read.recent_deliveries.len(), 50, "the limit holds");
    assert!(
        read.recent_deliveries
            .windows(2)
            .all(|pair| pair[0].queued_at >= pair[1].queued_at),
        "newest first"
    );
    let failed = read
        .recent_deliveries
        .iter()
        .find(|delivery| delivery.state == "failed")
        .expect("a failed delivery");
    assert_eq!((failed.attempts, failed.response_status), (3, Some(500)));
    assert_eq!(failed.rule_id, rule.id.to_string());

    let mut kinds: Vec<(&str, bool, Option<&str>)> = read
        .targets
        .iter()
        .map(|target| {
            (
                target.kind.as_str(),
                target.enabled,
                target.plugin_id.as_deref(),
            )
        })
        .collect();
    kinds.sort_unstable();
    assert_eq!(
        kinds,
        [("plugin", false, Some("ntfy")), ("webhook", true, None)]
    );
    assert_eq!(read.rules.len(), 1);
    assert_eq!(read.rules[0].events, ["package_completed"]);
    assert_eq!(read.rules[0].min_severity, "warning");
    assert!(read.rules[0].has_title_template);
    assert!(!read.rules[0].has_body_template);

    let text = serde_json::to_string(&read).expect("json");
    for canary in CANARIES {
        assert!(!text.contains(canary), "the read carries {canary}: {text}");
    }
}

#[tokio::test]
async fn the_queue_is_read_by_state_stage_transition_and_failure_without_a_name() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = temp_database(directory.path()).await;
    let root = routing_root(&database, directory.path()).await;
    let category = routing_category(&database, root, "Films", false).await;
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "package-name-canary".to_owned(),
            destination: "/srv/folder-canary".to_owned(),
            category_id: Some(category.id),
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    for index in 0..2 {
        database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id: package.id,
                source: format!("https://cdn.example/url-canary/{index}")
                    .parse()
                    .expect("URL"),
                file_name: format!("file-name-canary-{index}.mkv"),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: AuthProfileSelection::Auto,
                initial_state: rd_core::DownloadState::Queued,
                kind: rd_core::DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
    }
    let mut connection = raw(directory.path()).await;
    sqlx::query(
        "UPDATE downloads SET state = 'failed', last_error_json = ? WHERE rowid = \
         (SELECT MIN(rowid) FROM downloads)",
    )
    .bind(
        serde_json::json!({
            "category": { "kind": "permanent" },
            "message": "message-canary for https://cdn.example/url-canary",
            "code": "http.status_not_found",
        })
        .to_string(),
    )
    .execute(&mut connection)
    .await
    .expect("fail one download");

    let steps = [
        (PackageState::Downloading, None),
        (
            PackageState::Postprocessing,
            Some(PostprocessStage::Repairing),
        ),
        // A stage step inside one state is not a transition.
        (
            PackageState::Postprocessing,
            Some(PostprocessStage::Extracting),
        ),
        (PackageState::Failed, Some(PostprocessStage::Extracting)),
    ];
    for (state, stage) in steps {
        database
            .set_package_state(
                package.id,
                state,
                stage,
                Some(40),
                Some("current-file-canary.rar".to_owned()),
            )
            .await
            .expect("state");
    }

    let read = database.queue_diagnostics(50).await.expect("diagnostics");
    let transitions: Vec<(Option<&str>, &str, Option<&str>)> = read
        .recent_transitions
        .iter()
        .map(|step| {
            (
                step.previous.as_deref(),
                step.state.as_str(),
                step.stage.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        transitions,
        [
            (Some("postprocessing"), "failed", Some("extracting")),
            (Some("downloading"), "postprocessing", Some("repairing")),
            (Some("queued"), "downloading", None),
        ]
    );
    assert!(read.recent_transitions.iter().all(|step| {
        step.package_id == package.id.to_string()
            && step.category_id.as_deref() == Some(category.id.to_string().as_str())
    }));
    assert_eq!(
        database
            .queue_diagnostics(2)
            .await
            .expect("limited")
            .recent_transitions
            .len(),
        2,
        "the limit holds"
    );
    let by_state: Vec<(&str, u64)> = read
        .packages_by_state
        .iter()
        .map(|row| (row.value.as_str(), row.count))
        .collect();
    assert_eq!(by_state, [("failed", 1)]);
    let by_stage: Vec<(&str, u64)> = read
        .packages_by_stage
        .iter()
        .map(|row| (row.value.as_str(), row.count))
        .collect();
    assert_eq!(by_stage, [("extracting", 1)]);
    let downloads: Vec<(&str, Option<&str>, Option<&str>, u64)> = read
        .downloads
        .iter()
        .map(|row| {
            (
                row.state.as_str(),
                row.error_kind.as_deref(),
                row.error_code.as_deref(),
                row.count,
            )
        })
        .collect();
    assert_eq!(
        downloads,
        [
            (
                "failed",
                Some("permanent"),
                Some("http.status_not_found"),
                1
            ),
            ("queued", None, None, 1),
        ]
    );

    let text = serde_json::to_string(&read).expect("json");
    for canary in CANARIES {
        assert!(!text.contains(canary), "the read carries {canary}: {text}");
    }
}
