//! Resetting an automation's version history (RD-1280-01): only the version in force stays.

use anyhow::{Context, Result};
use chrono::Utc;
use rd_automation::VersionReset;
use rd_core::{AutomationId, EventEnvelope};
use sqlx::{Connection, SqliteConnection};

use crate::{
    automation_store::{AutomationRow, changed_event},
    error::StoreError,
    writer::insert_event,
};

/// Deletes every version but the one in force, with the runs of the deleted ones, and makes
/// the one in force version 1 — in one transaction, so a stop leaves the history as it was.
///
/// Refused while a run of an older version still has work ahead of it (queued, running or
/// waiting for a retry): deleting it would drop an action the engine is about to take, or is
/// taking, with no record that it was ever due. Runs of the version in force stay, and so do
/// their idempotency keys, which name the version's id rather than its number.
pub(crate) async fn reset_versions(
    connection: &mut SqliteConnection,
    id: AutomationId,
) -> Result<(VersionReset, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let automation = id.to_string();
    let current: Option<i64> = sqlx::query_scalar("SELECT version FROM automations WHERE id = ?")
        .bind(&automation)
        .fetch_optional(&mut *transaction)
        .await?;
    let current = current.context(StoreError::not_found("automation not found"))?;
    let current_id: String = sqlx::query_scalar(
        "SELECT id FROM automation_versions WHERE automation_id = ? AND version = ?",
    )
    .bind(&automation)
    .bind(current)
    .fetch_optional(&mut *transaction)
    .await?
    .context("the automation has no stored version in force")?;
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM automation_runs \
         WHERE automation_id = ? AND automation_version_id <> ? \
           AND state IN ('queued', 'running', 'retrying')",
    )
    .bind(&automation)
    .bind(&current_id)
    .fetch_one(&mut *transaction)
    .await?;
    if active > 0 {
        return Err(anyhow::Error::new(StoreError::wrong_state(
            "a run of an older version of this automation is still active",
        )));
    }
    // The runs first and by name, although the cascade would take them: the count is what the
    // answer reports, and it must be the rows that went, not an estimate made before.
    let removed_runs = sqlx::query(
        "DELETE FROM automation_runs WHERE automation_id = ? AND automation_version_id <> ?",
    )
    .bind(&automation)
    .bind(&current_id)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    let removed_versions =
        sqlx::query("DELETE FROM automation_versions WHERE automation_id = ? AND id <> ?")
            .bind(&automation)
            .bind(&current_id)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
    sqlx::query("UPDATE automation_versions SET version = 1 WHERE id = ?")
        .bind(&current_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE automations SET version = 1, updated_at = ? WHERE id = ?")
        .bind(Utc::now())
        .bind(&automation)
        .execute(&mut *transaction)
        .await?;
    let row = sqlx::query_as::<_, AutomationRow>(
        "SELECT id, name, enabled, version, created_at, updated_at FROM automations WHERE id = ?",
    )
    .bind(&automation)
    .fetch_one(&mut *transaction)
    .await?;
    let event = changed_event(id);
    insert_event(&mut transaction, &event).await?;
    rd_core::failpoint!("automation_versions.before_reset_committed", || {
        anyhow::anyhow!("crash point: the reset is written and not committed")
    });
    transaction.commit().await?;
    let reset = VersionReset {
        automation: row.try_into()?,
        removed_versions: u32::try_from(removed_versions).unwrap_or(u32::MAX),
        removed_runs: u32::try_from(removed_runs).unwrap_or(u32::MAX),
    };
    Ok((reset, event))
}
