//! Collision policies per category and package, the open `ask` prompts, and the content index
//! of finished files (RD-150-01). See `migrations/0099_collisions_and_content_index.sql`.

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rd_core::{CollisionDecision, CollisionPhase, CollisionPolicy, DownloadId, PackageId};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::parse_id;

/// The `scope_kind` of a category's policy.
pub const SCOPE_CATEGORY: &str = "category";
/// The `scope_kind` of a package's policy.
pub const SCOPE_PACKAGE: &str = "package";

/// A policy a category or a package holds of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionPolicyRow {
    /// [`SCOPE_CATEGORY`] or [`SCOPE_PACKAGE`].
    pub scope_kind: String,
    pub scope_id: String,
    pub policy: CollisionPolicy,
}

/// The two levels below the global setting, for one package.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CollisionPolicyLevels {
    pub package: Option<CollisionPolicy>,
    pub category: Option<CollisionPolicy>,
}

/// An `ask` that is waiting for an answer, or holding one until the next attempt uses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollisionPrompt {
    pub download_id: DownloadId,
    /// The name that was taken, inside the package folder.
    pub target_name: String,
    pub phase: CollisionPhase,
    /// Size of the file already there when the prompt was opened.
    pub existing_bytes: Option<u64>,
    pub decision: Option<CollisionDecision>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

/// What opening a prompt records.
#[derive(Clone, Debug)]
pub struct NewCollisionPrompt {
    pub download_id: DownloadId,
    pub target_name: String,
    pub phase: CollisionPhase,
    pub existing_bytes: Option<u64>,
}

/// One finished file in the content index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentIndexEntry {
    pub download_id: DownloadId,
    /// The stored word of the algorithm, as `downloads.computed_checksum_algorithm` spells it.
    pub algorithm: String,
    /// Lowercase hexadecimal.
    pub digest: String,
    pub size_bytes: u64,
    pub path: String,
    pub indexed_at: DateTime<Utc>,
    /// When a check last found the file missing; `None` while it is where `path` says.
    pub missing_since: Option<DateTime<Utc>>,
}

fn timestamp(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .context("parse stored timestamp")?
        .with_timezone(&Utc))
}

fn policy(value: &str) -> Result<CollisionPolicy> {
    CollisionPolicy::parse(value).with_context(|| format!("unknown collision policy {value:?}"))
}

/// Every policy whose category or package still exists.
pub(crate) async fn list_collision_policies(pool: &SqlitePool) -> Result<Vec<CollisionPolicyRow>> {
    let rows = sqlx::query_as::<_, (String, String, String)>(
        "SELECT c.scope_kind, c.scope_id, c.policy FROM collision_policies c \
         WHERE (c.scope_kind = 'category' AND EXISTS (SELECT 1 FROM categories WHERE id = c.scope_id)) \
            OR (c.scope_kind = 'package' AND EXISTS (SELECT 1 FROM packages WHERE id = c.scope_id)) \
         ORDER BY c.scope_kind, c.scope_id",
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|(scope_kind, scope_id, value)| {
            Ok(CollisionPolicyRow {
                scope_kind,
                scope_id,
                policy: policy(&value)?,
            })
        })
        .collect()
}

/// The package's own policy and its category's, `None` where a level has none.
pub(crate) async fn collision_policy_levels(
    pool: &SqlitePool,
    package_id: PackageId,
) -> Result<CollisionPolicyLevels> {
    let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT \
           (SELECT policy FROM collision_policies WHERE scope_kind = 'package' AND scope_id = p.id), \
           (SELECT policy FROM collision_policies WHERE scope_kind = 'category' AND scope_id = p.category_id) \
         FROM packages p WHERE p.id = ?",
    )
    .bind(package_id.to_string())
    .fetch_optional(pool)
    .await?;
    let Some((package, category)) = row else {
        return Ok(CollisionPolicyLevels::default());
    };
    Ok(CollisionPolicyLevels {
        package: package.as_deref().map(policy).transpose()?,
        category: category.as_deref().map(policy).transpose()?,
    })
}

/// Sets or clears (`None`) the policy of one category or package.
pub(crate) async fn set_collision_policy(
    connection: &mut SqliteConnection,
    scope_kind: &str,
    scope_id: &str,
    value: Option<CollisionPolicy>,
) -> Result<()> {
    match value {
        None => {
            sqlx::query("DELETE FROM collision_policies WHERE scope_kind = ? AND scope_id = ?")
                .bind(scope_kind)
                .bind(scope_id)
                .execute(connection)
                .await?;
        }
        Some(value) => {
            sqlx::query(
                "INSERT INTO collision_policies (scope_kind, scope_id, policy, updated_at) \
                 VALUES (?, ?, ?, ?) \
                 ON CONFLICT(scope_kind, scope_id) DO UPDATE SET \
                   policy = excluded.policy, updated_at = excluded.updated_at",
            )
            .bind(scope_kind)
            .bind(scope_id)
            .bind(value.as_str())
            .bind(timestamp(&Utc::now()))
            .execute(connection)
            .await?;
        }
    }
    Ok(())
}

#[derive(FromRow)]
struct PromptRow {
    download_id: String,
    target_name: String,
    phase: String,
    existing_bytes: Option<i64>,
    decision: Option<String>,
    created_at: String,
    decided_at: Option<String>,
}

impl TryFrom<PromptRow> for CollisionPrompt {
    type Error = anyhow::Error;

    fn try_from(row: PromptRow) -> Result<Self> {
        Ok(Self {
            download_id: parse_id(&row.download_id)?,
            target_name: row.target_name,
            phase: CollisionPhase::parse(&row.phase)
                .with_context(|| format!("unknown collision phase {:?}", row.phase))?,
            existing_bytes: row
                .existing_bytes
                .and_then(|value| u64::try_from(value).ok()),
            decision: row
                .decision
                .as_deref()
                .map(|value| {
                    CollisionDecision::parse(value)
                        .with_context(|| format!("unknown collision decision {value:?}"))
                })
                .transpose()?,
            created_at: parse_time(&row.created_at)?,
            decided_at: row.decided_at.as_deref().map(parse_time).transpose()?,
        })
    }
}

const PROMPT_COLUMNS: &str =
    "download_id, target_name, phase, existing_bytes, decision, created_at, decided_at";

pub(crate) async fn collision_prompt(
    pool: &SqlitePool,
    download_id: DownloadId,
) -> Result<Option<CollisionPrompt>> {
    sqlx::query_as::<_, PromptRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {PROMPT_COLUMNS} FROM collision_prompts WHERE download_id = ?"
    )))
    .bind(download_id.to_string())
    .fetch_optional(pool)
    .await?
    .map(CollisionPrompt::try_from)
    .transpose()
}

pub(crate) async fn list_collision_prompts(pool: &SqlitePool) -> Result<Vec<CollisionPrompt>> {
    sqlx::query_as::<_, PromptRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {PROMPT_COLUMNS} FROM collision_prompts ORDER BY created_at, download_id"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(CollisionPrompt::try_from)
    .collect()
}

/// Opens a prompt, or reopens one: a new collision is a new question, so an answer that was
/// given to an earlier one does not carry over.
pub(crate) async fn open_collision_prompt(
    connection: &mut SqliteConnection,
    prompt: NewCollisionPrompt,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO collision_prompts \
           (download_id, target_name, phase, existing_bytes, decision, created_at, decided_at) \
         VALUES (?, ?, ?, ?, NULL, ?, NULL) \
         ON CONFLICT(download_id) DO UPDATE SET \
           target_name = excluded.target_name, phase = excluded.phase, \
           existing_bytes = excluded.existing_bytes, decision = NULL, \
           created_at = excluded.created_at, decided_at = NULL",
    )
    .bind(prompt.download_id.to_string())
    .bind(&prompt.target_name)
    .bind(prompt.phase.as_str())
    .bind(
        prompt
            .existing_bytes
            .map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
    )
    .bind(timestamp(&Utc::now()))
    .execute(connection)
    .await?;
    Ok(())
}

/// Records the answer. `false` when there is no prompt for that download.
pub(crate) async fn decide_collision_prompt(
    connection: &mut SqliteConnection,
    download_id: DownloadId,
    decision: CollisionDecision,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE collision_prompts SET decision = ?, decided_at = ? WHERE download_id = ?",
    )
    .bind(decision.as_str())
    .bind(timestamp(&Utc::now()))
    .bind(download_id.to_string())
    .execute(connection)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn clear_collision_prompt(
    connection: &mut SqliteConnection,
    download_id: DownloadId,
) -> Result<()> {
    sqlx::query("DELETE FROM collision_prompts WHERE download_id = ?")
        .bind(download_id.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

#[derive(FromRow)]
struct IndexRow {
    download_id: String,
    algorithm: String,
    digest: String,
    size_bytes: i64,
    path: String,
    indexed_at: String,
    missing_since: Option<String>,
}

impl TryFrom<IndexRow> for ContentIndexEntry {
    type Error = anyhow::Error;

    fn try_from(row: IndexRow) -> Result<Self> {
        Ok(Self {
            download_id: parse_id(&row.download_id)?,
            algorithm: row.algorithm,
            digest: row.digest,
            size_bytes: u64::try_from(row.size_bytes).unwrap_or_default(),
            path: row.path,
            indexed_at: parse_time(&row.indexed_at)?,
            missing_since: row.missing_since.as_deref().map(parse_time).transpose()?,
        })
    }
}

const INDEX_COLUMNS: &str =
    "download_id, algorithm, digest, size_bytes, path, indexed_at, missing_since";

pub(crate) async fn content_index_entry(
    pool: &SqlitePool,
    download_id: DownloadId,
) -> Result<Option<ContentIndexEntry>> {
    sqlx::query_as::<_, IndexRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {INDEX_COLUMNS} FROM content_index WHERE download_id = ?"
    )))
    .bind(download_id.to_string())
    .fetch_optional(pool)
    .await?
    .map(ContentIndexEntry::try_from)
    .transpose()
}

/// Every entry with this digest, missing ones included; the caller says which it wants.
pub(crate) async fn content_index_matches(
    pool: &SqlitePool,
    algorithm: &str,
    digest: &str,
) -> Result<Vec<ContentIndexEntry>> {
    sqlx::query_as::<_, IndexRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {INDEX_COLUMNS} FROM content_index WHERE algorithm = ? AND digest = ? \
         ORDER BY indexed_at, download_id"
    )))
    .bind(algorithm)
    .bind(digest.to_ascii_lowercase())
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(ContentIndexEntry::try_from)
    .collect()
}

pub(crate) async fn list_content_index(pool: &SqlitePool) -> Result<Vec<ContentIndexEntry>> {
    sqlx::query_as::<_, IndexRow>(sqlx::AssertSqlSafe(format!(
        "SELECT {INDEX_COLUMNS} FROM content_index ORDER BY indexed_at, download_id"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(ContentIndexEntry::try_from)
    .collect()
}

/// Records (or replaces) the entry of one finished file; it is present again, whatever an
/// earlier check said.
pub(crate) async fn index_content(
    connection: &mut SqliteConnection,
    download_id: DownloadId,
    algorithm: &str,
    digest: &str,
    size_bytes: u64,
    path: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO content_index \
           (download_id, algorithm, digest, size_bytes, path, indexed_at, missing_since) \
         VALUES (?, ?, ?, ?, ?, ?, NULL) \
         ON CONFLICT(download_id) DO UPDATE SET \
           algorithm = excluded.algorithm, digest = excluded.digest, \
           size_bytes = excluded.size_bytes, path = excluded.path, \
           indexed_at = excluded.indexed_at, missing_since = NULL",
    )
    .bind(download_id.to_string())
    .bind(algorithm)
    .bind(digest.to_ascii_lowercase())
    .bind(i64::try_from(size_bytes).unwrap_or(i64::MAX))
    .bind(path)
    .bind(timestamp(&Utc::now()))
    .execute(connection)
    .await?;
    Ok(())
}

/// Points an entry at the place its file was moved to. Nothing happens for a download that
/// has no entry.
pub(crate) async fn move_indexed_content(
    connection: &mut SqliteConnection,
    download_id: DownloadId,
    path: &str,
) -> Result<()> {
    sqlx::query("UPDATE content_index SET path = ?, missing_since = NULL WHERE download_id = ?")
        .bind(path)
        .bind(download_id.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

/// Marks entries missing or present again, in one transaction. A missing entry keeps the
/// time it was first found missing.
pub(crate) async fn mark_indexed_content(
    connection: &mut SqliteConnection,
    changes: Vec<(DownloadId, bool)>,
) -> Result<()> {
    let now = timestamp(&Utc::now());
    let mut transaction = connection.begin().await?;
    for (download_id, missing) in changes {
        if missing {
            sqlx::query(
                "UPDATE content_index SET missing_since = COALESCE(missing_since, ?) \
                 WHERE download_id = ?",
            )
            .bind(&now)
            .bind(download_id.to_string())
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query("UPDATE content_index SET missing_since = NULL WHERE download_id = ?")
                .bind(download_id.to_string())
                .execute(&mut *transaction)
                .await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

/// Drops the entries of every other download that point at `path`: after an overwrite the file
/// there holds this download's bytes, and their digest no longer describes it.
pub(crate) async fn forget_indexed_path(
    connection: &mut SqliteConnection,
    path: &str,
    except: DownloadId,
) -> Result<u64> {
    let result = sqlx::query("DELETE FROM content_index WHERE path = ? AND download_id != ?")
        .bind(path)
        .bind(except.to_string())
        .execute(connection)
        .await?;
    Ok(result.rows_affected())
}
