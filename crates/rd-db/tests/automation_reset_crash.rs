//! The automation version reset's crash binary. `automation_versions.before_reset_committed`
//! (RD-1280-01, recovery matrix): the transaction that keeps only the version in force stops
//! after the older versions and their runs are deleted and the rest renumbered, before it commits.
#![cfg(feature = "failpoints")]

use std::path::Path;

use rd_automation::{Action, ConditionNode, RunState, Trigger};
use rd_core::{AutomationId, EventId, failpoint::FailpointGuard};
use rd_db::{Database, NewAutomation, NewRun};

async fn open(directory: &Path) -> Database {
    Database::open(directory.join("rdownloader.sqlite3"))
        .await
        .expect("database")
}

fn definition() -> NewAutomation {
    NewAutomation {
        name: "Tidy up".to_owned(),
        enabled: true,
        trigger: Trigger::PackageCompleted,
        schedule: None,
        condition: ConditionNode::Always,
        actions: vec![Action::PausePackage],
    }
}

/// Three saves, one finished run per version.
async fn history(database: &Database) -> AutomationId {
    let mut id = None;
    for _ in 0..3 {
        id = Some(
            database
                .upsert_automation(id, definition())
                .await
                .expect("save")
                .id,
        );
    }
    let id = id.expect("saved");
    for version in database.automation_versions(id).await.expect("versions") {
        let event = EventId::new();
        database
            .queue_automation_run(NewRun {
                automation_id: id,
                automation_version_id: version.id,
                event_id: event.to_string(),
                package_id: None,
                idempotency_key: rd_automation::idempotency_key(version.id, &event),
            })
            .await
            .expect("queue");
    }
    for run in database.automation_runs(Some(id), 500).await.expect("runs") {
        database
            .record_automation_attempt(run.id, RunState::Completed, 1, 1, None, None)
            .await
            .expect("finish");
    }
    id
}

/// A stop before the commit leaves every version and run and the number in force; the reset
/// done again keeps exactly the version in force as version 1, and that survives a restart.
#[tokio::test]
async fn a_reset_stopped_before_its_commit_leaves_the_history_whole_and_runs_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let (id, current) = {
        let database = open(data).await;
        let id = history(&database).await;
        let current = database.automation_versions(id).await.expect("versions")[0].id;
        let guard = FailpointGuard::once("automation_versions.before_reset_committed");
        database
            .reset_automation_versions(id)
            .await
            .expect_err("the reset stops at the crash point");
        assert!(guard.fired());
        (id, current)
    };
    {
        let database = open(data).await;
        let versions = database.automation_versions(id).await.expect("versions");
        assert_eq!(
            versions
                .iter()
                .map(|version| version.version)
                .collect::<Vec<_>>(),
            vec![3, 2, 1],
            "every version is still there, numbered as before"
        );
        assert_eq!(
            database
                .automation_runs(Some(id), 500)
                .await
                .expect("runs")
                .len(),
            3
        );
        let automation = database
            .list_automations()
            .await
            .expect("automations")
            .into_iter()
            .find(|automation| automation.id == id)
            .expect("kept");
        assert_eq!(automation.version, 3);
        let reset = database.reset_automation_versions(id).await.expect("reset");
        assert_eq!((reset.removed_versions, reset.removed_runs), (2, 2));
    }
    let database = open(data).await;
    let versions = database.automation_versions(id).await.expect("versions");
    assert_eq!(versions.len(), 1);
    assert_eq!((versions[0].id, versions[0].version), (current, 1));
    let runs = database.automation_runs(Some(id), 500).await.expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].automation_version_id, current);
}
