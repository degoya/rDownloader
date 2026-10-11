//! The time trigger's half of the automation engine (RD-1240-10).
//!
//! A clock is not a bus event, so the time trigger has a loop of its own: every few seconds it
//! asks each enabled time-triggered automation whether a slot of its schedule is due, and
//! queues a run for it into the same run table every other trigger feeds. From there the run
//! is the ordinary run loop's, with its retries and its recovery.
//!
//! Two promises, both from the owner (2026-10-10):
//!
//! * **No catch-up.** A slot is due for [`rd_automation::GRACE_SECONDS`] after its time; one the
//!   service was down for longer is not run after the restart.
//! * **No double run.** The run's idempotency key names the automation and the slot, and the run
//!   table refuses a second row under it — across a restart inside the window too.

use chrono::{DateTime, TimeZone, Utc};
use rd_automation::{EventContext, Trigger};

/// How often the schedules are looked at; well inside the grace window.
pub(crate) const TICK: std::time::Duration = std::time::Duration::from_secs(15);

/// Queues a run for every time-triggered automation whose slot is due at `now`, read in
/// `zone` — the service's own in production (`chrono::Local`), a fixed one in a test. Answers
/// how many runs were queued; a slot that already has its run counts none.
///
/// # Errors
///
/// The store's error.
pub(crate) async fn queue_due<Tz: TimeZone>(
    database: &rd_db::Database,
    now: DateTime<Utc>,
    zone: &Tz,
) -> anyhow::Result<usize> {
    let mut queued = 0;
    for version in database.active_automation_versions().await? {
        if version.trigger != Trigger::Schedule {
            continue;
        }
        let Some(slot) = version
            .schedule
            .as_ref()
            .and_then(|schedule| schedule.due_slot(now, zone))
        else {
            continue;
        };
        // A slot from before this definition existed is not one it was asked to run: saving
        // an automation at 06:00:30 for "every day at six" waits for tomorrow.
        if slot.at < version.created_at {
            continue;
        }
        // A clock carries no download, so the condition is judged against nothing, as it is
        // for a storage threshold.
        if !version.condition.matches(&EventContext::default()) {
            continue;
        }
        let fresh = database
            .queue_automation_run(rd_db::NewRun {
                automation_id: version.automation_id,
                automation_version_id: version.id,
                event_id: format!("schedule:{}", slot.label()),
                package_id: None,
                idempotency_key: rd_automation::schedule_key(version.automation_id, &slot),
            })
            .await?;
        if fresh {
            queued += 1;
        }
    }
    Ok(queued)
}

#[cfg(test)]
#[path = "automation_schedule_tests.rs"]
mod tests;
