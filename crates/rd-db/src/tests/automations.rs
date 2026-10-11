//! Automations: versions, runs and retries.

use chrono::{Duration, Utc};

use crate::Database;

/// Builds a definition with one action and no condition.
fn new_automation(name: &str, enabled: bool) -> crate::NewAutomation {
    crate::NewAutomation {
        name: name.to_owned(),
        enabled,
        trigger: rd_automation::Trigger::PackageCompleted,
        schedule: None,
        condition: rd_automation::ConditionNode::Always,
        actions: vec![rd_automation::Action::PausePackage],
    }
}

#[tokio::test]
async fn saving_an_automation_writes_a_new_version_every_time() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("automations.sqlite"))
        .await
        .expect("database");

    let created = database
        .upsert_automation(None, new_automation("Pause big files", false))
        .await
        .expect("create");
    assert_eq!(created.version, 1);
    assert!(!created.enabled);

    let updated = database
        .upsert_automation(Some(created.id), new_automation("Pause big files", true))
        .await
        .expect("update");
    assert_eq!(updated.version, 2, "an edit must not rewrite version 1");
    assert!(updated.enabled);

    // Both versions are still readable, because a run points at the one it started under.
    let versions = database
        .automation_versions(created.id)
        .await
        .expect("versions");
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].version, 2, "newest first");
    assert_eq!(versions[1].version, 1);
}

#[tokio::test]
async fn only_enabled_automations_are_active() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("active.sqlite"))
        .await
        .expect("database");

    let off = database
        .upsert_automation(None, new_automation("Disabled", false))
        .await
        .expect("create");
    let on = database
        .upsert_automation(None, new_automation("Enabled", true))
        .await
        .expect("create");

    let active = database.active_automation_versions().await.expect("active");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].automation_id, on.id);

    // Enabling one brings exactly its current version into force.
    database
        .set_automation_enabled(off.id, true)
        .await
        .expect("enable");
    let active = database.active_automation_versions().await.expect("active");
    assert_eq!(active.len(), 2);
}

#[tokio::test]
async fn the_same_event_queues_a_run_only_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("idempotent.sqlite"))
        .await
        .expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Once", true))
        .await
        .expect("create");
    let version = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    let key = rd_automation::idempotency_key(version.id, &event);
    let run = crate::NewRun {
        automation_id: automation.id,
        automation_version_id: version.id,
        event_id: event.to_string(),
        package_id: None,
        idempotency_key: key,
    };

    assert!(
        database
            .queue_automation_run(run.clone())
            .await
            .expect("queue")
    );
    // Replaying the same event after a crash must not run the automation a second time.
    assert!(
        !database
            .queue_automation_run(run)
            .await
            .expect("queue again")
    );
    assert_eq!(
        database
            .automation_runs(Some(automation.id), 10)
            .await
            .expect("runs")
            .len(),
        1
    );
}

#[tokio::test]
async fn an_edited_automation_may_see_an_event_the_previous_version_handled() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("reedit.sqlite"))
        .await
        .expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Twice", true))
        .await
        .expect("create");
    let first = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    assert!(
        database
            .queue_automation_run(crate::NewRun {
                automation_id: automation.id,
                automation_version_id: first.id,
                event_id: event.to_string(),
                package_id: None,
                idempotency_key: rd_automation::idempotency_key(first.id, &event),
            })
            .await
            .expect("queue")
    );

    database
        .upsert_automation(Some(automation.id), new_automation("Twice", true))
        .await
        .expect("edit");
    let second = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    assert_ne!(first.id, second.id);

    // The key is per version, not per automation: an edited definition is entitled to act
    // on an event the previous one already saw.
    assert!(
        database
            .queue_automation_run(crate::NewRun {
                automation_id: automation.id,
                automation_version_id: second.id,
                event_id: event.to_string(),
                package_id: None,
                idempotency_key: rd_automation::idempotency_key(second.id, &event),
            })
            .await
            .expect("queue")
    );
}

#[tokio::test]
async fn a_run_interrupted_mid_flight_is_queued_again_after_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("recover.sqlite");
    let database = Database::open(&path).await.expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Recoverable", true))
        .await
        .expect("create");
    let version = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    database
        .queue_automation_run(crate::NewRun {
            automation_id: automation.id,
            automation_version_id: version.id,
            event_id: event.to_string(),
            package_id: None,
            idempotency_key: rd_automation::idempotency_key(version.id, &event),
        })
        .await
        .expect("queue");
    let run = database
        .automation_runs(Some(automation.id), 1)
        .await
        .expect("runs")
        .remove(0);

    // The process dies here: the run is claimed but its outcome was never recorded.
    database
        .record_automation_attempt(run.id, rd_automation::RunState::Running, 0, 1, None, None)
        .await
        .expect("claim");
    drop(database);

    let database = Database::open(&path).await.expect("reopen");
    assert_eq!(
        database.recover_automation_runs().await.expect("recover"),
        1
    );
    let due = database.due_automation_runs(Utc::now()).await.expect("due");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].state, rd_automation::RunState::Queued);
}

#[tokio::test]
async fn a_retry_only_becomes_due_once_its_backoff_has_passed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("due.sqlite"))
        .await
        .expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Retrying", true))
        .await
        .expect("create");
    let version = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    database
        .queue_automation_run(crate::NewRun {
            automation_id: automation.id,
            automation_version_id: version.id,
            event_id: event.to_string(),
            package_id: None,
            idempotency_key: rd_automation::idempotency_key(version.id, &event),
        })
        .await
        .expect("queue");
    let run = database
        .automation_runs(Some(automation.id), 1)
        .await
        .expect("runs")
        .remove(0);

    let later = Utc::now() + Duration::minutes(5);
    database
        .record_automation_attempt(
            run.id,
            rd_automation::RunState::Retrying,
            0,
            1,
            Some(later),
            Some("target refused".to_owned()),
        )
        .await
        .expect("record");

    assert!(
        database
            .due_automation_runs(Utc::now())
            .await
            .expect("due")
            .is_empty(),
        "a retry must not be picked up before its backoff has passed"
    );
    assert_eq!(
        database
            .due_automation_runs(later + Duration::seconds(1))
            .await
            .expect("due")
            .len(),
        1
    );
}

#[tokio::test]
async fn deleting_an_automation_takes_its_versions_and_runs_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("delete.sqlite"))
        .await
        .expect("database");

    let automation = database
        .upsert_automation(None, new_automation("Temporary", true))
        .await
        .expect("create");
    let version = database
        .active_automation_versions()
        .await
        .expect("active")
        .remove(0);
    let event = rd_core::EventId::new();
    database
        .queue_automation_run(crate::NewRun {
            automation_id: automation.id,
            automation_version_id: version.id,
            event_id: event.to_string(),
            package_id: None,
            idempotency_key: rd_automation::idempotency_key(version.id, &event),
        })
        .await
        .expect("queue");

    database
        .delete_automation(automation.id)
        .await
        .expect("delete");
    assert!(database.list_automations().await.expect("list").is_empty());
    assert!(
        database
            .automation_runs(None, 10)
            .await
            .expect("runs")
            .is_empty(),
        "runs outlived the automation they belong to"
    );
}

#[tokio::test]
async fn a_time_trigger_keeps_its_schedule_across_a_reopen() {
    // RD-1240-10: the schedule is part of the version, read back after a restart as written.
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("automations.sqlite");
    let schedule = rd_automation::Schedule::Cron {
        expression: "0 6 * * 1-5".to_owned(),
    };
    {
        let database = Database::open(&path).await.expect("database");
        let mut input = new_automation("Start at six", true);
        input.trigger = rd_automation::Trigger::Schedule;
        input.schedule = Some(schedule.clone());
        input.actions = vec![rd_automation::Action::StartQueue];
        database.upsert_automation(None, input).await.expect("save");
        let plain = new_automation("Plain", true);
        database.upsert_automation(None, plain).await.expect("save");
    }
    let database = Database::open(&path).await.expect("reopen");
    let versions = database.active_automation_versions().await.expect("active");
    let timed = versions
        .iter()
        .find(|version| version.trigger == rd_automation::Trigger::Schedule)
        .expect("time-triggered version");
    assert_eq!(timed.schedule.as_ref(), Some(&schedule));
    assert_eq!(timed.actions, vec![rd_automation::Action::StartQueue]);
    assert!(
        versions
            .iter()
            .filter(|version| version.trigger != rd_automation::Trigger::Schedule)
            .all(|version| version.schedule.is_none())
    );
}
