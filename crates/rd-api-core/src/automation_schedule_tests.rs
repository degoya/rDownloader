use chrono::{DateTime, Duration, DurationRound, Timelike, Utc};
use rd_automation::{Action, ConditionNode, Schedule, Trigger};

use super::queue_due;

/// A whole minute at least a minute after the real clock, so a definition saved now existed
/// before it however slow the test machine is.
fn next_minute() -> DateTime<Utc> {
    Utc::now()
        .duration_trunc(Duration::minutes(1))
        .expect("truncate")
        + Duration::minutes(2)
}

/// A daily cron expression naming `slot`'s minute, read in UTC.
fn daily_at(slot: DateTime<Utc>) -> Schedule {
    Schedule::Cron {
        expression: format!("{} {} * * *", slot.minute(), slot.hour()),
    }
}

async fn save(database: &rd_db::Database, schedule: Schedule) -> rd_core::AutomationId {
    database
        .upsert_automation(
            None,
            rd_db::NewAutomation {
                name: "Start the queue".to_owned(),
                enabled: true,
                trigger: Trigger::Schedule,
                schedule: Some(schedule),
                condition: ConditionNode::Always,
                actions: vec![Action::StartQueue],
            },
        )
        .await
        .expect("automation")
        .id
}

async fn runs(database: &rd_db::Database, automation: rd_core::AutomationId) -> Vec<String> {
    database
        .automation_runs(Some(automation), 10)
        .await
        .expect("runs")
        .into_iter()
        .map(|run| run.event_id)
        .collect()
}

#[tokio::test]
async fn a_time_trigger_runs_at_its_time_and_once_across_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("schedule.sqlite3");
    let slot = next_minute();
    let automation = {
        let database = rd_db::Database::open(&path).await.expect("database");
        let automation = save(&database, daily_at(slot)).await;
        // Not before its time.
        let early = queue_due(&database, slot - Duration::seconds(5), &Utc)
            .await
            .expect("tick");
        assert_eq!(early, 0);
        assert!(runs(&database, automation).await.is_empty());
        // At its time, once, however often the loop looks inside the window.
        for seconds in [2, 17, 32] {
            queue_due(&database, slot + Duration::seconds(seconds), &Utc)
                .await
                .expect("tick");
        }
        let label = slot.format("schedule:%Y-%m-%dT%H:%M").to_string();
        assert_eq!(runs(&database, automation).await, vec![label]);
        automation
    };
    // The service stops and starts again inside the window: the slot's run is in the table,
    // and its key refuses a second one.
    let database = rd_db::Database::open(&path).await.expect("reopen");
    let after_restart = queue_due(&database, slot + Duration::seconds(50), &Utc)
        .await
        .expect("tick");
    assert_eq!(
        after_restart, 0,
        "the slot ran a second time after the restart"
    );
    assert_eq!(runs(&database, automation).await.len(), 1);
    // The next day's slot is a slot of its own.
    let tomorrow = queue_due(
        &database,
        slot + Duration::days(1) + Duration::seconds(3),
        &Utc,
    )
    .await
    .expect("tick");
    assert_eq!(tomorrow, 1);
}

#[tokio::test]
async fn a_slot_missed_while_the_service_was_down_is_not_caught_up() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("missed.sqlite3"))
        .await
        .expect("database");
    let slot = next_minute();
    let automation = save(&database, daily_at(slot)).await;
    // The first look after the slot comes long after its window.
    let late = slot + Duration::seconds(rd_automation::GRACE_SECONDS + 60);
    assert_eq!(queue_due(&database, late, &Utc).await.expect("tick"), 0);
    assert!(runs(&database, automation).await.is_empty());
}

#[tokio::test]
async fn a_disabled_or_unmatched_time_trigger_runs_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("disabled.sqlite3"))
        .await
        .expect("database");
    let slot = next_minute();
    let automation = save(&database, daily_at(slot)).await;
    database
        .set_automation_enabled(automation, false)
        .await
        .expect("disable");
    assert_eq!(
        queue_due(&database, slot + Duration::seconds(5), &Utc)
            .await
            .expect("tick"),
        0
    );
    // A condition on a download's name never holds for a clock, which carries none.
    let named = database
        .upsert_automation(
            None,
            rd_db::NewAutomation {
                name: "Only for a name".to_owned(),
                enabled: true,
                trigger: Trigger::Schedule,
                schedule: Some(daily_at(slot)),
                condition: ConditionNode::Predicate {
                    predicate: rd_automation::Predicate {
                        field: rd_automation::Field::Name,
                        operator: rd_automation::Operator::Contains,
                        value: "x".to_owned(),
                    },
                },
                actions: vec![Action::StartQueue],
            },
        )
        .await
        .expect("automation")
        .id;
    queue_due(&database, slot + Duration::seconds(5), &Utc)
        .await
        .expect("tick");
    assert!(runs(&database, named).await.is_empty());
}
