//! The audit log store (RD-110-03): rows survive a restart, the table refuses an update, the
//! filters answer what the viewer asks, retention deletes whole rows in bounded batches, and
//! restoring a configuration backup leaves the log alone.

use std::collections::BTreeMap;

use chrono::{Duration, Utc};
use rd_core::{AuditAction, AuditActorKind, AuditOutcome};
use rd_db::{AuditQuery, Database, NewAuditRecord};
use tempfile::TempDir;

async fn database(directory: &TempDir) -> Database {
    Database::open(directory.path().join("audit.sqlite"))
        .await
        .expect("database")
}

fn record(action: AuditAction, outcome: AuditOutcome) -> NewAuditRecord {
    NewAuditRecord::new(action, outcome)
}

fn query() -> AuditQuery {
    AuditQuery {
        limit: 100,
        ..AuditQuery::default()
    }
}

#[tokio::test]
async fn records_survive_a_restart_and_come_back_newest_first() {
    let directory = tempfile::tempdir().expect("tempdir");
    {
        let database = database(&directory).await;
        database
            .append_audit_records(vec![
                record(AuditAction::LoginFailed, AuditOutcome::Failure),
                record(AuditAction::LoginSucceeded, AuditOutcome::Success),
            ])
            .await
            .expect("append");
    }
    let database = database(&directory).await;
    let records = database.query_audit_records(&query()).await.expect("query");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].action, AuditAction::LoginSucceeded);
    assert_eq!(records[1].action, AuditAction::LoginFailed);
    assert_eq!(database.count_audit_records().await.expect("count"), 2);
}

#[tokio::test]
async fn the_table_refuses_an_update_whatever_issues_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .append_audit_record(record(AuditAction::TokenCreated, AuditOutcome::Success))
        .await
        .expect("append");

    // Straight at the file, past every facade this crate offers: the append-only guarantee
    // has to hold against a statement nobody in the workspace would write, because the point
    // of it is somebody who is not in the workspace.
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite://{}",
        directory.path().join("audit.sqlite").display()
    ))
    .await
    .expect("pool");
    let refused = sqlx::query("UPDATE audit_records SET action = 'logout' WHERE id = 1")
        .execute(&pool)
        .await;
    let error = refused.expect_err("an update must be refused");
    assert!(
        error.to_string().contains("append-only"),
        "unexpected refusal: {error}"
    );

    let records = database.query_audit_records(&query()).await.expect("query");
    assert_eq!(records[0].action, AuditAction::TokenCreated);
}

#[tokio::test]
async fn the_filters_answer_what_the_viewer_asks() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut login = record(AuditAction::LoginFailed, AuditOutcome::Failure);
    login.actor_kind = AuditActorKind::Anonymous;
    login.client_address = Some("203.0.113.9".to_owned());
    login.trace_id = Some("a".repeat(32));
    let mut deletion = record(AuditAction::DownloadDeleted, AuditOutcome::Success);
    deletion.actor_kind = AuditActorKind::Session;
    deletion.actor_id = Some("session-1".to_owned());
    deletion.target_kind = Some("download".to_owned());
    deletion.target_id = Some("42".to_owned());
    deletion.details = BTreeMap::from([("keep_files".to_owned(), "false".to_owned())]);
    database
        .append_audit_records(vec![login, deletion])
        .await
        .expect("append");

    let by_action = database
        .query_audit_records(&AuditQuery {
            action: Some(AuditAction::DownloadDeleted),
            ..query()
        })
        .await
        .expect("query");
    assert_eq!(by_action.len(), 1);
    assert_eq!(by_action[0].target_id.as_deref(), Some("42"));
    assert_eq!(
        by_action[0].details.get("keep_files").map(String::as_str),
        Some("false")
    );

    let by_outcome = database
        .query_audit_records(&AuditQuery {
            outcome: Some(AuditOutcome::Failure),
            ..query()
        })
        .await
        .expect("query");
    assert_eq!(by_outcome.len(), 1);
    assert_eq!(by_outcome[0].client_address.as_deref(), Some("203.0.113.9"));

    let by_actor = database
        .query_audit_records(&AuditQuery {
            actor_kind: Some(AuditActorKind::Session),
            actor_id: Some("session-1".to_owned()),
            ..query()
        })
        .await
        .expect("query");
    assert_eq!(by_actor.len(), 1);

    let by_trace = database
        .query_audit_records(&AuditQuery {
            trace_id: Some("a".repeat(32)),
            ..query()
        })
        .await
        .expect("query");
    assert_eq!(by_trace.len(), 1);
    assert_eq!(by_trace[0].action, AuditAction::LoginFailed);

    let by_target = database
        .query_audit_records(&AuditQuery {
            target_kind: Some("download".to_owned()),
            ..query()
        })
        .await
        .expect("query");
    assert_eq!(by_target.len(), 1);
}

#[tokio::test]
async fn retention_deletes_whole_records_in_bounded_batches() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut batch = Vec::new();
    for index in 0..250 {
        let mut entry = record(AuditAction::TokenUsed, AuditOutcome::Success);
        entry.actor_id = Some(format!("token-{index}"));
        batch.push(entry);
    }
    database.append_audit_records(batch).await.expect("append");

    let first = database
        .prune_audit_records(50, None, 40)
        .await
        .expect("prune");
    assert_eq!(first.deleted, 40, "a batch never exceeds its cap");
    assert_eq!(first.remaining_over_cap, 160);
    assert_eq!(database.count_audit_records().await.expect("count"), 210);

    let mut guard = 0;
    while database
        .prune_audit_records(50, None, 40)
        .await
        .expect("prune")
        .remaining_over_cap
        > 0
    {
        guard += 1;
        assert!(guard < 100, "the prune loop did not converge");
    }
    assert_eq!(database.count_audit_records().await.expect("count"), 50);

    // The survivors are the newest: retention removes the oldest whole rows and nothing else.
    let left = database.query_audit_records(&query()).await.expect("query");
    assert_eq!(left.len(), 50);
    assert_eq!(left[0].actor_id.as_deref(), Some("token-249"));
}

#[tokio::test]
async fn an_expired_record_goes_whatever_the_count_says() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    let mut old = record(AuditAction::SettingsChanged, AuditOutcome::Success);
    old.recorded_at = Utc::now() - Duration::days(400);
    database
        .append_audit_records(vec![
            old,
            record(AuditAction::SettingsChanged, AuditOutcome::Success),
        ])
        .await
        .expect("append");

    let cutoff = Utc::now() - Duration::days(365);
    let report = database
        .prune_audit_records(1_000_000, Some(cutoff), 100)
        .await
        .expect("prune");
    assert_eq!(report.deleted, 1);
    assert_eq!(database.count_audit_records().await.expect("count"), 1);
}

#[tokio::test]
async fn restoring_a_configuration_backup_leaves_the_audit_log_alone() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = database(&directory).await;
    database
        .append_audit_record(record(AuditAction::BackupRestored, AuditOutcome::Success))
        .await
        .expect("append");
    let before = database.count_audit_records().await.expect("count");

    database
        .replace_config(rd_db::ConfigReplacement::default())
        .await
        .expect("restore");

    assert_eq!(
        database.count_audit_records().await.expect("count"),
        before,
        "a restore must not be able to erase the record of itself"
    );
}
