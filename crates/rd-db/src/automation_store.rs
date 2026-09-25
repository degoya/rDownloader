//! Persistence of automations, their versions and the run history (RD-090-04).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rd_automation::{Action, Automation, AutomationVersion, ConditionNode, Run, RunState, Trigger};
use rd_core::{AutomationId, AutomationRunId, AutomationVersionId, EventEnvelope, EventKind};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{error::StoreError, parse_id, writer::insert_event};

/// Runs kept per automation. Older ones are trimmed on insert, as the plugin execution log
/// does: a history nobody prunes is a table that grows until it is the biggest one there is.
const MAX_RUNS_PER_AUTOMATION: i64 = 200;

/// The editable part of an automation; the id and the version counter stay with the row.
#[derive(Clone, Debug)]
pub struct NewAutomation {
    pub name: String,
    pub enabled: bool,
    pub trigger: Trigger,
    pub condition: ConditionNode,
    pub actions: Vec<Action>,
}

/// A run about to be queued.
#[derive(Clone, Debug)]
pub struct NewRun {
    pub automation_id: AutomationId,
    pub automation_version_id: AutomationVersionId,
    pub event_id: String,
    pub idempotency_key: String,
    pub package_id: Option<rd_core::PackageId>,
}

#[derive(FromRow)]
struct AutomationRow {
    id: String,
    name: String,
    enabled: bool,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct VersionRow {
    id: String,
    automation_id: String,
    version: i64,
    trigger_kind: String,
    condition_json: String,
    actions_json: String,
    created_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct RunRow {
    id: String,
    automation_id: String,
    automation_version_id: String,
    event_id: String,
    package_id: Option<String>,
    state: String,
    action_index: i64,
    attempt: i64,
    next_attempt_at: Option<DateTime<Utc>>,
    message: Option<String>,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<Automation>> {
    sqlx::query_as::<_, AutomationRow>(
        "SELECT id, name, enabled, version, created_at, updated_at FROM automations \
         ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// The definition currently in force for every enabled automation.
///
/// Enabled only: a disabled automation must not start new runs, and filtering here rather
/// than in the engine keeps that decision in one place.
pub(crate) async fn active_versions(pool: &SqlitePool) -> Result<Vec<AutomationVersion>> {
    sqlx::query_as::<_, VersionRow>(
        "SELECT v.id, v.automation_id, v.version, v.trigger_kind, v.condition_json, \
                v.actions_json, v.created_at \
         FROM automation_versions v \
         JOIN automations a ON a.id = v.automation_id AND a.version = v.version \
         WHERE a.enabled = 1",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn version(
    pool: &SqlitePool,
    id: AutomationVersionId,
) -> Result<Option<AutomationVersion>> {
    sqlx::query_as::<_, VersionRow>(
        "SELECT id, automation_id, version, trigger_kind, condition_json, actions_json, \
                created_at FROM automation_versions WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub(crate) async fn versions(
    pool: &SqlitePool,
    automation_id: AutomationId,
) -> Result<Vec<AutomationVersion>> {
    sqlx::query_as::<_, VersionRow>(
        "SELECT id, automation_id, version, trigger_kind, condition_json, actions_json, \
                created_at FROM automation_versions WHERE automation_id = ? \
         ORDER BY version DESC",
    )
    .bind(automation_id.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn runs(
    pool: &SqlitePool,
    automation_id: Option<AutomationId>,
    limit: u32,
) -> Result<Vec<Run>> {
    let rows = match automation_id {
        Some(id) => {
            sqlx::query_as::<_, RunRow>(
                "SELECT id, automation_id, automation_version_id, event_id, package_id, state, \
                        action_index, attempt, next_attempt_at, message, started_at, finished_at \
                 FROM automation_runs WHERE automation_id = ? ORDER BY started_at DESC LIMIT ?",
            )
            .bind(id.to_string())
            .bind(i64::from(limit))
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, RunRow>(
                "SELECT id, automation_id, automation_version_id, event_id, package_id, state, \
                        action_index, attempt, next_attempt_at, message, started_at, finished_at \
                 FROM automation_runs ORDER BY started_at DESC LIMIT ?",
            )
            .bind(i64::from(limit))
            .fetch_all(pool)
            .await?
        }
    };
    rows.into_iter().map(TryInto::try_into).collect()
}

/// Runs that are waiting to start or whose retry is due.
pub(crate) async fn due_runs(pool: &SqlitePool, now: DateTime<Utc>) -> Result<Vec<Run>> {
    sqlx::query_as::<_, RunRow>(
        "SELECT id, automation_id, automation_version_id, event_id, package_id, state, \
                action_index, attempt, next_attempt_at, message, started_at, finished_at \
         FROM automation_runs \
         WHERE state = 'queued' OR (state = 'retrying' AND next_attempt_at <= ?) \
         ORDER BY started_at",
    )
    .bind(now)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// Stores an automation and, when the definition changed, a new version of it.
///
/// A new version is written on every save rather than only on a semantic change: the run
/// history points at versions, and rewriting one in place would make an old run's record
/// describe rules it never ran under.
pub(crate) async fn upsert(
    connection: &mut SqliteConnection,
    id: Option<AutomationId>,
    input: NewAutomation,
) -> Result<(Automation, EventEnvelope)> {
    let now = Utc::now();
    let mut transaction = connection.begin().await?;
    let id = match id {
        Some(id) => id,
        None => {
            let id = AutomationId::new();
            sqlx::query(
                "INSERT INTO automations (id, name, enabled, version, created_at, updated_at) \
                 VALUES (?, ?, ?, 0, ?, ?)",
            )
            .bind(id.to_string())
            .bind(&input.name)
            .bind(input.enabled)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await?;
            id
        }
    };
    let version: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM automation_versions WHERE automation_id = ?",
    )
    .bind(id.to_string())
    .fetch_one(&mut *transaction)
    .await?;
    let version_id = AutomationVersionId::new();
    sqlx::query(
        "INSERT INTO automation_versions (id, automation_id, version, trigger_kind, \
                condition_json, actions_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(version_id.to_string())
    .bind(id.to_string())
    .bind(version)
    .bind(
        serde_json::to_string(&input.trigger)?
            .trim_matches('"')
            .to_owned(),
    )
    .bind(serde_json::to_string(&input.condition)?)
    .bind(serde_json::to_string(&input.actions)?)
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE automations SET name = ?, enabled = ?, version = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&input.name)
    .bind(input.enabled)
    .bind(version)
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *transaction)
    .await?;
    let row = sqlx::query_as::<_, AutomationRow>(
        "SELECT id, name, enabled, version, created_at, updated_at FROM automations WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_one(&mut *transaction)
    .await?;
    let event = changed_event(id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((row.try_into()?, event))
}

/// Enables or disables an automation without touching its definition.
///
/// Disabling stops new runs and leaves the ones in flight alone: an action that has already
/// been started is not made safer by abandoning it half way.
pub(crate) async fn set_enabled(
    connection: &mut SqliteConnection,
    id: AutomationId,
    enabled: bool,
) -> Result<(Automation, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let result = sqlx::query("UPDATE automations SET enabled = ?, updated_at = ? WHERE id = ?")
        .bind(enabled)
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    (result.rows_affected() == 1)
        .then_some(())
        .context(StoreError::not_found("automation not found"))?;
    let row = sqlx::query_as::<_, AutomationRow>(
        "SELECT id, name, enabled, version, created_at, updated_at FROM automations WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_one(&mut *transaction)
    .await?;
    let event = changed_event(id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((row.try_into()?, event))
}

/// Removes an automation, and says so only if a row actually went.
///
/// The event is written inside the same transaction as the delete, so a removal that is
/// rolled back cannot leave an announcement behind telling clients to refetch something that
/// is still there.
pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: AutomationId,
) -> Result<EventEnvelope> {
    let mut transaction = connection.begin().await?;
    let result = sqlx::query("DELETE FROM automations WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
    (result.rows_affected() == 1)
        .then_some(())
        .context(StoreError::not_found("automation not found"))?;
    let event = changed_event(id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

/// What an automation write announces.
///
/// The id alone, as the notification store does it: the name, the trigger, the condition
/// tree and the action list are the automation's contents, and an event that carried them
/// would hand every `Config`-scoped stream subscriber the rule bodies of the installation.
/// A client learns which automation to refetch, which is what it needs.
fn changed_event(id: AutomationId) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::AutomationChanged,
        serde_json::json!({ "automation_id": id }),
    )
}

/// Queues a run. `false` means the idempotency key was already taken.
pub(crate) async fn queue_run(connection: &mut SqliteConnection, input: NewRun) -> Result<bool> {
    let now = Utc::now();
    let mut transaction = connection.begin().await?;
    let result = sqlx::query(
        "INSERT OR IGNORE INTO automation_runs (id, automation_id, automation_version_id, \
                event_id, package_id, idempotency_key, state, action_index, attempt, started_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'queued', 0, 0, ?)",
    )
    .bind(AutomationRunId::new().to_string())
    .bind(input.automation_id.to_string())
    .bind(input.automation_version_id.to_string())
    .bind(&input.event_id)
    .bind(input.package_id.map(|id| id.to_string()))
    .bind(&input.idempotency_key)
    .bind(now)
    .execute(&mut *transaction)
    .await?;
    let queued = result.rows_affected() == 1;
    if queued {
        sqlx::query(
            "DELETE FROM automation_runs WHERE automation_id = ? AND id NOT IN (\
                SELECT id FROM automation_runs WHERE automation_id = ? \
                ORDER BY started_at DESC LIMIT ?)",
        )
        .bind(input.automation_id.to_string())
        .bind(input.automation_id.to_string())
        .bind(MAX_RUNS_PER_AUTOMATION)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(queued)
}

/// Records the outcome of one attempt.
pub(crate) async fn record_attempt(
    connection: &mut SqliteConnection,
    id: AutomationRunId,
    state: RunState,
    action_index: u32,
    attempt: u32,
    next_attempt_at: Option<DateTime<Utc>>,
    message: Option<String>,
) -> Result<()> {
    let finished = matches!(state, RunState::Completed | RunState::Failed);
    sqlx::query(
        "UPDATE automation_runs SET state = ?, action_index = ?, attempt = ?, \
                next_attempt_at = ?, message = ?, finished_at = ? WHERE id = ?",
    )
    .bind(serde_json::to_string(&state)?.trim_matches('"').to_owned())
    .bind(i64::from(action_index))
    .bind(i64::from(attempt))
    .bind(next_attempt_at)
    .bind(message)
    .bind(finished.then(Utc::now))
    .bind(id.to_string())
    .execute(&mut *connection)
    .await?;
    Ok(())
}

/// Moves runs that were mid-flight when the service stopped back into the queue.
///
/// A `running` row means the process died between claiming a run and recording its outcome.
/// Re-queuing rather than failing it is right because the actions are the ones the author
/// asked for, and the alternative — dropping it — loses work with no record of why.
pub(crate) async fn recover(connection: &mut SqliteConnection) -> Result<u64> {
    let result = sqlx::query(
        "UPDATE automation_runs SET state = 'queued', next_attempt_at = NULL \
         WHERE state = 'running'",
    )
    .execute(&mut *connection)
    .await?;
    Ok(result.rows_affected())
}

impl TryFrom<AutomationRow> for Automation {
    type Error = anyhow::Error;

    fn try_from(row: AutomationRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            name: row.name,
            enabled: row.enabled,
            version: u32::try_from(row.version).unwrap_or(1),
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

impl TryFrom<VersionRow> for AutomationVersion {
    type Error = anyhow::Error;

    fn try_from(row: VersionRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            automation_id: parse_id(&row.automation_id)?,
            version: u32::try_from(row.version).unwrap_or(1),
            trigger: serde_json::from_str(&format!("\"{}\"", row.trigger_kind))
                .context("unknown automation trigger")?,
            condition: serde_json::from_str(&row.condition_json)?,
            actions: serde_json::from_str(&row.actions_json)?,
            created_at: row.created_at,
        })
    }
}

impl TryFrom<RunRow> for Run {
    type Error = anyhow::Error;

    fn try_from(row: RunRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            automation_id: parse_id(&row.automation_id)?,
            automation_version_id: parse_id(&row.automation_version_id)?,
            event_id: row.event_id,
            package_id: row.package_id.as_deref().map(parse_id).transpose()?,
            state: serde_json::from_str(&format!("\"{}\"", row.state))
                .context("unknown automation run state")?,
            action_index: u32::try_from(row.action_index).unwrap_or(0),
            attempt: u32::try_from(row.attempt).unwrap_or(0),
            next_attempt_at: row.next_attempt_at,
            message: row.message,
            started_at: row.started_at,
            finished_at: row.finished_at,
        })
    }
}
