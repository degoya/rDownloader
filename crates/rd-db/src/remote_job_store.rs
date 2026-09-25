//! Persistence of jobs that run at a provider (RD-107-06).
//!
//! One row per job, and the row is the whole reason the eleventh world could be additive: a
//! plugin is instantiated fresh for every call and remembers nothing, so what has to survive a
//! restart survives here. `docs/adr/0003-a-job-that-runs-at-the-provider.md` argues the split.
//!
//! The order of writes in this file is load-bearing rather than incidental. A row exists, with
//! its content key, **before** anything is handed to any provider, and the identifier the
//! provider answers with is written as the very next thing that happens. That is what makes a
//! duplicate preventable at a provider whose submit is not idempotent.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{
    AccountId, CollectorPackageId, EventEnvelope, EventKind, RemoteJob, RemoteJobFile, RemoteJobId,
    RemoteJobSourceKind, RemoteJobState,
};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

/// A job being claimed, before the provider has been asked for anything.
///
/// Claimed and not created: the point of writing this first is that the unique index on
/// `(account_id, content_key)` answers "is this already ours" without a network call.
#[derive(Clone, Debug)]
pub struct ClaimRemoteJob {
    pub id: RemoteJobId,
    pub account_id: AccountId,
    pub plugin_id: String,
    pub content_key: String,
    pub source_kind: RemoteJobSourceKind,
    /// The magnet or plain address as UTF-8, or the container's bytes.
    pub source: Vec<u8>,
    pub package_id: Option<CollectorPackageId>,
}

/// What one poll, one submit or one answer changed about a job.
///
/// An update rather than an upsert on purpose: the caller of a sweep knows the new state and
/// nothing else, and an upsert would make it restate the source and the content key — the two
/// values that must never move once written — in order to record a percentage.
#[derive(Clone, Debug, Default)]
pub struct AdvanceRemoteJob {
    pub state: Option<RemoteJobState>,
    pub remote_id: Option<String>,
    pub job_state: Option<Option<String>>,
    pub entries: Option<Vec<RemoteJobFile>>,
    pub chosen: Option<Vec<u32>>,
    pub progress_permille: Option<Option<u16>>,
    pub message: Option<Option<String>>,
    pub code: Option<Option<String>>,
    pub next_poll_at: Option<Option<DateTime<Utc>>>,
    pub package_id: Option<CollectorPackageId>,
    /// Adds one to the count of times the provider has been asked to create this job.
    pub count_submit_attempt: bool,
    /// Records that the provider has been asked what it already holds for the content key.
    pub adoption_checked: bool,
}

/// Writes the row that stands for one remote job, before the provider is asked for anything.
///
/// Refuses rather than replaces when the account already has a job for this content: a second
/// row would become a second `submit`, which at every provider this world was designed for is
/// a second torrent in somebody's account. The caller is expected to look the existing row up
/// and show it instead.
pub(crate) async fn claim(
    connection: &mut SqliteConnection,
    input: ClaimRemoteJob,
) -> Result<(RemoteJob, EventEnvelope)> {
    let now = Utc::now();
    let affected = sqlx::query(
        "INSERT INTO remote_jobs \
         (id, account_id, plugin_id, content_key, remote_id, state, source_kind, source, \
          submit_attempts, adoption_checked, package_id, entries, chosen, progress_permille, \
          message, code, next_poll_at, created_at, updated_at, job_state) \
         VALUES (?, ?, ?, ?, NULL, ?, ?, ?, 0, 0, ?, NULL, NULL, NULL, NULL, NULL, ?, ?, ?, NULL) \
         ON CONFLICT(account_id, content_key) DO NOTHING",
    )
    .bind(input.id.to_string())
    .bind(input.account_id.to_string())
    .bind(&input.plugin_id)
    .bind(&input.content_key)
    .bind(RemoteJobState::Submitting.as_str())
    .bind(input.source_kind.as_str())
    .bind(&input.source)
    .bind(input.package_id.map(|id| id.to_string()))
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    if affected == 0 {
        bail!("this account already has a remote job for that content");
    }
    let job = RemoteJob {
        id: input.id,
        account_id: input.account_id,
        plugin_id: input.plugin_id,
        content_key: input.content_key,
        remote_id: None,
        state: RemoteJobState::Submitting,
        source_kind: input.source_kind,
        submit_attempts: 0,
        adoption_checked: false,
        package_id: input.package_id,
        entries: Vec::new(),
        chosen: Vec::new(),
        progress_permille: None,
        message: None,
        code: None,
        next_poll_at: Some(now),
        created_at: now,
        updated_at: now,
        job_state: None,
    };
    let event = changed(&job);
    Ok((job, event))
}

/// Advances one job, refusing a state its current one may not reach.
///
/// The refusal is the point. A poll answer that arrives after somebody deleted the job would
/// otherwise resurrect it, and nothing may ever put a row back into `submitting` — the source
/// was handed over once, and a second submit is the duplicate this design exists to prevent.
pub(crate) async fn advance(
    connection: &mut SqliteConnection,
    id: RemoteJobId,
    input: AdvanceRemoteJob,
) -> Result<(RemoteJob, EventEnvelope)> {
    let Some(current) = fetch(&mut *connection, id).await? else {
        bail!("no remote job {id}");
    };
    if let Some(next) = input.state
        && !current.state.may_advance_to(next)
    {
        bail!(
            "a remote job in {} may not move to {}",
            current.state.as_str(),
            next.as_str()
        );
    }
    // The identifier is written once and never rewritten. A provider answering with a second
    // one means a second job was created, and overwriting would lose the first for ever.
    if let Some(remote_id) = &input.remote_id
        && let Some(existing) = &current.remote_id
        && existing != remote_id
    {
        bail!("remote job {id} already names another job at the provider");
    }
    let now = Utc::now();
    sqlx::query(
        "UPDATE remote_jobs SET \
         state = COALESCE(?, state), \
         remote_id = COALESCE(?, remote_id), \
         job_state = CASE WHEN ? THEN ? ELSE job_state END, \
         entries = COALESCE(?, entries), \
         chosen = COALESCE(?, chosen), \
         progress_permille = CASE WHEN ? THEN ? ELSE progress_permille END, \
         message = CASE WHEN ? THEN ? ELSE message END, \
         code = CASE WHEN ? THEN ? ELSE code END, \
         next_poll_at = CASE WHEN ? THEN ? ELSE next_poll_at END, \
         package_id = COALESCE(?, package_id), \
         submit_attempts = submit_attempts + ?, \
         adoption_checked = CASE WHEN ? THEN 1 ELSE adoption_checked END, \
         updated_at = ? \
         WHERE id = ?",
    )
    .bind(input.state.map(RemoteJobState::as_str))
    .bind(input.remote_id.as_deref())
    .bind(input.job_state.is_some())
    .bind(input.job_state.clone().flatten())
    .bind(
        input
            .entries
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
    )
    .bind(
        input
            .chosen
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
    )
    .bind(input.progress_permille.is_some())
    .bind(input.progress_permille.flatten().map(i64::from))
    .bind(input.message.is_some())
    .bind(input.message.clone().flatten())
    .bind(input.code.is_some())
    .bind(input.code.clone().flatten())
    .bind(input.next_poll_at.is_some())
    .bind(input.next_poll_at.flatten())
    .bind(input.package_id.map(|id| id.to_string()))
    .bind(i64::from(u8::from(input.count_submit_attempt)))
    .bind(input.adoption_checked)
    .bind(now)
    .bind(id.to_string())
    .execute(&mut *connection)
    .await?;
    let Some(job) = fetch(&mut *connection, id).await? else {
        bail!("remote job {id} vanished while it was being advanced");
    };
    let event = changed(&job);
    Ok((job, event))
}

/// One job by its own identifier.
pub(crate) async fn get(pool: &SqlitePool, id: RemoteJobId) -> Result<Option<RemoteJob>> {
    sqlx::query_as::<_, JobRow>(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

/// The job one account already has for a content key, if any.
///
/// The read half of the duplicate guard: a person pasting the same magnet twice is shown the
/// job they already started instead of starting a second one.
pub(crate) async fn by_content(
    pool: &SqlitePool,
    account_id: AccountId,
    content_key: &str,
) -> Result<Option<RemoteJob>> {
    sqlx::query_as::<_, JobRow>(&format!(
        "{SELECT} WHERE account_id = ? AND content_key = ?"
    ))
    .bind(account_id.to_string())
    .bind(content_key)
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

/// Every job that is still asked about and whose next poll is due, oldest first.
///
/// `awaiting_choice` is deliberately not in the list. Nothing at the provider changes until a
/// person answers, and polling in the meantime would spend an account's request budget on
/// re-reading a question nobody has got to yet.
pub(crate) async fn due(pool: &SqlitePool, now: DateTime<Utc>) -> Result<Vec<RemoteJob>> {
    sqlx::query_as::<_, JobRow>(&format!(
        "{SELECT} WHERE state IN ('submitting', 'preparing', 'working') \
         AND (next_poll_at IS NULL OR next_poll_at <= ?) ORDER BY created_at"
    ))
    .bind(now)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// Every job this installation knows about, newest first.
///
/// What the interface lists. Across accounts rather than per account, because a person reading
/// "is anything running at a provider" is asking about their installation and not about one
/// account at a time; the row names its own account.
pub(crate) async fn list_all(pool: &SqlitePool) -> Result<Vec<RemoteJob>> {
    sqlx::query_as::<_, JobRow>(&format!("{SELECT} ORDER BY created_at DESC"))
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(TryInto::try_into)
        .collect()
}

/// Every job of one account, newest first.
pub(crate) async fn list(pool: &SqlitePool, account_id: AccountId) -> Result<Vec<RemoteJob>> {
    sqlx::query_as::<_, JobRow>(&format!(
        "{SELECT} WHERE account_id = ? ORDER BY created_at DESC"
    ))
    .bind(account_id.to_string())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

/// The source a job was claimed with, so a restart can offer the very same bytes again.
pub(crate) async fn source(pool: &SqlitePool, id: RemoteJobId) -> Result<Option<Vec<u8>>> {
    let row: Option<(Vec<u8>,)> = sqlx::query_as("SELECT source FROM remote_jobs WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| row.0))
}

/// Removes one job's row, and nothing at the provider.
///
/// The counterpart of `discard` and deliberately a different act: this forgets what the
/// installation knows about a job, while what the provider holds stays exactly where it is.
/// ADR 0003 puts the provider-side deletion behind one explicit, confirmed request, so a row
/// leaving somebody's list must never reach one.
///
/// The event carries `removed` rather than a state, because there is no longer a row to name
/// one: a listener that patched a state onto a job it no longer has would put it back.
pub(crate) async fn remove(
    connection: &mut SqliteConnection,
    id: RemoteJobId,
) -> Result<(bool, EventEnvelope)> {
    let account_id = fetch(&mut *connection, id).await?.map(|job| job.account_id);
    let affected = sqlx::query("DELETE FROM remote_jobs WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *connection)
        .await?
        .rows_affected();
    let event = EventEnvelope::new(
        EventKind::RemoteJobChanged,
        serde_json::json!({
            "entity": "remote_job",
            "id": id,
            "account_id": account_id,
            "removed": true,
        }),
    );
    Ok((affected > 0, event))
}

fn changed(job: &RemoteJob) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::RemoteJobChanged,
        serde_json::json!({
            "entity": "remote_job",
            "id": job.id,
            "account_id": job.account_id,
            "state": job.state,
        }),
    )
}

async fn fetch(connection: &mut SqliteConnection, id: RemoteJobId) -> Result<Option<RemoteJob>> {
    sqlx::query_as::<_, JobRow>(&format!("{SELECT} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(&mut *connection)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

const SELECT: &str = "SELECT id, account_id, plugin_id, content_key, remote_id, state, \
     source_kind, submit_attempts, adoption_checked, package_id, entries, chosen, \
     progress_permille, message, code, next_poll_at, created_at, updated_at, job_state \
     FROM remote_jobs";

#[derive(FromRow)]
struct JobRow {
    id: String,
    account_id: String,
    plugin_id: String,
    content_key: String,
    remote_id: Option<String>,
    state: String,
    source_kind: String,
    submit_attempts: i64,
    adoption_checked: bool,
    package_id: Option<String>,
    entries: Option<String>,
    chosen: Option<String>,
    progress_permille: Option<i64>,
    message: Option<String>,
    code: Option<String>,
    next_poll_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    job_state: Option<String>,
}

impl TryFrom<JobRow> for RemoteJob {
    type Error = anyhow::Error;

    fn try_from(row: JobRow) -> Result<Self> {
        let Some(state) = RemoteJobState::from_str_value(&row.state) else {
            bail!(
                "remote job {} holds an unknown state `{}`",
                row.id,
                row.state
            );
        };
        let Some(source_kind) = RemoteJobSourceKind::from_str_value(&row.source_kind) else {
            bail!(
                "remote job {} holds an unknown source kind `{}`",
                row.id,
                row.source_kind
            );
        };
        Ok(Self {
            id: row.id.parse()?,
            account_id: row.account_id.parse()?,
            plugin_id: row.plugin_id,
            content_key: row.content_key,
            remote_id: row.remote_id,
            state,
            source_kind,
            submit_attempts: u32::try_from(row.submit_attempts.max(0)).unwrap_or(u32::MAX),
            adoption_checked: row.adoption_checked,
            package_id: row.package_id.map(|id| id.parse()).transpose()?,
            entries: match row.entries {
                Some(raw) => serde_json::from_str(&raw)?,
                None => Vec::new(),
            },
            chosen: match row.chosen {
                Some(raw) => serde_json::from_str(&raw)?,
                None => Vec::new(),
            },
            progress_permille: row
                .progress_permille
                .and_then(|value| u16::try_from(value.clamp(0, 1_000)).ok()),
            message: row.message,
            code: row.code,
            next_poll_at: row.next_poll_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            job_state: row.job_state,
        })
    }
}
