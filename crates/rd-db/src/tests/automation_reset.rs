//! Resetting an automation's version history (RD-1280-01).

use rd_automation::{AutomationVersion, RunState};
use rd_core::{AutomationId, AutomationRunId};

use crate::{Database, StoreErrorKind, store_kind};

fn definition(name: &str) -> crate::NewAutomation {
    crate::NewAutomation {
        name: name.to_owned(),
        enabled: true,
        trigger: rd_automation::Trigger::PackageCompleted,
        schedule: None,
        condition: rd_automation::ConditionNode::Always,
        actions: vec![rd_automation::Action::PausePackage],
    }
}

async fn open(directory: &tempfile::TempDir) -> Database {
    Database::open(directory.path().join("reset.sqlite"))
        .await
        .expect("database")
}

/// An automation saved `saves` times, and its versions newest first.
async fn saved(database: &Database, saves: usize) -> (AutomationId, Vec<AutomationVersion>) {
    let mut id = None;
    for _ in 0..saves {
        let stored = database
            .upsert_automation(id, definition("Tidy up"))
            .await
            .expect("save");
        id = Some(stored.id);
    }
    let id = id.expect("saved at least once");
    let versions = database.automation_versions(id).await.expect("versions");
    (id, versions)
}

/// Queues a run of `version` and leaves it in `state`.
async fn run(database: &Database, version: &AutomationVersion, state: RunState) -> AutomationRunId {
    let event = rd_core::EventId::new();
    database
        .queue_automation_run(crate::NewRun {
            automation_id: version.automation_id,
            automation_version_id: version.id,
            event_id: event.to_string(),
            package_id: None,
            idempotency_key: rd_automation::idempotency_key(version.id, &event),
        })
        .await
        .expect("queue");
    let id = database
        .automation_runs(Some(version.automation_id), 500)
        .await
        .expect("runs")
        .into_iter()
        .find(|run| run.automation_version_id == version.id && run.state == RunState::Queued)
        .expect("the queued run")
        .id;
    if state != RunState::Queued {
        database
            .record_automation_attempt(id, state, 0, 1, None, None)
            .await
            .expect("state");
    }
    id
}

#[tokio::test]
async fn a_reset_keeps_only_the_version_in_force_as_version_one_with_its_runs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let (id, versions) = saved(&database, 3).await;
    let current = versions[0].clone();
    run(&database, &versions[2], RunState::Completed).await;
    run(&database, &versions[1], RunState::Failed).await;
    run(&database, &versions[1], RunState::Completed).await;
    let kept = run(&database, &current, RunState::Completed).await;

    let reset = database.reset_automation_versions(id).await.expect("reset");
    assert_eq!(reset.removed_versions, 2);
    assert_eq!(reset.removed_runs, 3);
    assert_eq!(reset.automation.version, 1);

    let left = database.automation_versions(id).await.expect("versions");
    assert_eq!(left.len(), 1, "only the version in force stays");
    assert_eq!(left[0].id, current.id, "the same definition, not a copy");
    assert_eq!(left[0].version, 1);
    let runs = database.automation_runs(Some(id), 500).await.expect("runs");
    assert_eq!(
        runs.iter().map(|run| run.id).collect::<Vec<_>>(),
        vec![kept],
        "the runs of the older versions went with them"
    );
    let active = database.active_automation_versions().await.expect("active");
    assert_eq!(active.len(), 1, "the version in force is still in force");
    assert_eq!(active[0].id, current.id);

    // The next save continues from 1.
    let next = database
        .upsert_automation(Some(id), definition("Tidy up"))
        .await
        .expect("save");
    assert_eq!(next.version, 2);
}

#[tokio::test]
async fn an_automation_with_one_version_resets_to_itself() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let (id, _) = saved(&database, 1).await;
    let reset = database.reset_automation_versions(id).await.expect("reset");
    assert_eq!((reset.removed_versions, reset.removed_runs), (0, 0));
    assert_eq!(reset.automation.version, 1);
}

#[tokio::test]
async fn an_active_run_of_an_older_version_refuses_the_reset_and_changes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    for state in [RunState::Queued, RunState::Running, RunState::Retrying] {
        let (id, versions) = saved(&database, 2).await;
        let active = run(&database, &versions[1], state).await;

        let error = database
            .reset_automation_versions(id)
            .await
            .expect_err("refused while a run of version 1 is active");
        assert_eq!(
            store_kind(&error),
            Some(StoreErrorKind::WrongState),
            "{state:?}"
        );
        assert_eq!(
            database
                .automation_versions(id)
                .await
                .expect("versions")
                .len(),
            2,
            "{state:?}: nothing is deleted"
        );
        assert_eq!(
            database
                .automation_runs(Some(id), 500)
                .await
                .expect("runs")
                .len(),
            1
        );

        // Once the run has finished, the reset goes through.
        database
            .record_automation_attempt(active, RunState::Failed, 0, 2, None, None)
            .await
            .expect("finish");
        let reset = database.reset_automation_versions(id).await.expect("reset");
        assert_eq!((reset.removed_versions, reset.removed_runs), (1, 1));
    }
}

#[tokio::test]
async fn an_active_run_of_the_version_in_force_does_not_stop_the_reset() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let (id, versions) = saved(&database, 2).await;
    let running = run(&database, &versions[0], RunState::Running).await;
    let reset = database.reset_automation_versions(id).await.expect("reset");
    assert_eq!((reset.removed_versions, reset.removed_runs), (1, 0));
    let runs = database.automation_runs(Some(id), 500).await.expect("runs");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, running);
    assert_eq!(runs[0].state, RunState::Running);
}

#[tokio::test]
async fn resetting_an_unknown_automation_is_not_found() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(&directory).await;
    let error = database
        .reset_automation_versions(AutomationId::new())
        .await
        .expect_err("unknown");
    assert_eq!(store_kind(&error), Some(StoreErrorKind::NotFound));
}
