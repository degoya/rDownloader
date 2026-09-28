//! Plugin repositories, their replay floors, the signing keys they withdrew and what was
//! installed from them (RD-140-01, migration 0097).
//!
//! The rows are configuration and history; the signed indexes themselves live as files beside
//! the database and are re-verified at every start, the way the tool manifest's cache is.

use anyhow::Result;
use chrono::Utc;
use rd_core::{EventEnvelope, EventKind};
use serde::{Deserialize, Serialize};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::writer::insert_event;

/// The id of the built-in repository, seeded by the migration.
pub const OFFICIAL_REPOSITORY_ID: &str = "official";

/// One configured repository.
#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct PluginRepository {
    pub id: String,
    /// `official` or `third_party`.
    pub kind: String,
    pub name: String,
    /// The index address; `None` for the official repository, whose address is compiled in.
    pub url: Option<String>,
    pub key_id: Option<String>,
    /// Base64 Ed25519 key the person approved.
    pub public_key: Option<String>,
    pub fingerprint: Option<String>,
    pub enabled: bool,
    /// The highest index sequence accepted from this repository.
    pub sequence: Option<i64>,
    pub issued_at: Option<String>,
    pub last_checked_at: Option<String>,
    pub last_success_at: Option<String>,
    /// Stable code of the last refresh's failure.
    pub last_error: Option<String>,
    pub created_at: String,
}

impl PluginRepository {
    /// Whether this is the built-in repository.
    #[must_use]
    pub fn is_official(&self) -> bool {
        self.kind == "official"
    }
}

/// A third-party repository whose key the person has just approved.
#[derive(Clone, Debug)]
pub struct NewPluginRepository {
    pub id: String,
    pub name: String,
    pub url: String,
    pub key_id: String,
    pub public_key: String,
    pub fingerprint: String,
}

/// What one refresh of one repository ended with.
#[derive(Clone, Debug)]
pub enum RepositoryCheck {
    /// An index verified and was adopted; the floor rises to `sequence`, never falls.
    Accepted { sequence: i64, issued_at: String },
    /// The refresh failed with this stable code; the floor and the cache stay as they were.
    Failed { code: String },
}

/// A plugin signing key one repository withdrew.
#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct PluginWithdrawnKey {
    pub fingerprint: String,
    pub key_id: String,
    pub repository_id: String,
    pub withdrawn_at: String,
}

/// One installed version and the repository it came from.
#[derive(Clone, Debug, Deserialize, FromRow, PartialEq, Eq, Serialize)]
pub struct PluginRepositoryInstall {
    pub plugin_id: String,
    pub version: String,
    pub digest: String,
    pub repository_id: String,
    pub installed_at: String,
}

const COLUMNS: &str = "id, kind, name, url, key_id, public_key, fingerprint, enabled, sequence, \
     issued_at, last_checked_at, last_success_at, last_error, created_at";

pub(crate) async fn list_plugin_repositories(pool: &SqlitePool) -> Result<Vec<PluginRepository>> {
    // The official repository first, then the others in the order they were added.
    let rows = sqlx::query_as::<_, PluginRepository>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM plugin_repositories \
         ORDER BY kind = 'official' DESC, created_at, id"
    )))
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub(crate) async fn plugin_repository(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<PluginRepository>> {
    let row = sqlx::query_as::<_, PluginRepository>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM plugin_repositories WHERE id = ?"
    )))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub(crate) async fn list_plugin_withdrawn_keys(
    pool: &SqlitePool,
) -> Result<Vec<PluginWithdrawnKey>> {
    let rows = sqlx::query_as::<_, PluginWithdrawnKey>(
        "SELECT fingerprint, key_id, repository_id, withdrawn_at \
         FROM plugin_withdrawn_keys ORDER BY withdrawn_at DESC, fingerprint",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub(crate) async fn list_plugin_repository_installs(
    pool: &SqlitePool,
) -> Result<Vec<PluginRepositoryInstall>> {
    let rows = sqlx::query_as::<_, PluginRepositoryInstall>(
        "SELECT plugin_id, version, digest, repository_id, installed_at \
         FROM plugin_repository_installs ORDER BY installed_at DESC, plugin_id, version",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Records an approved third-party repository. A second one at the same address is refused by
/// the unique index, which surfaces as a constraint error the caller maps to a conflict.
pub(crate) async fn insert_plugin_repository(
    connection: &mut SqliteConnection,
    input: NewPluginRepository,
) -> Result<(PluginRepository, EventEnvelope)> {
    let created_at = Utc::now().to_rfc3339();
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO plugin_repositories \
           (id, kind, name, url, key_id, public_key, fingerprint, enabled, created_at) \
         VALUES (?, 'third_party', ?, ?, ?, ?, ?, 1, ?)",
    )
    .bind(&input.id)
    .bind(&input.name)
    .bind(&input.url)
    .bind(&input.key_id)
    .bind(&input.public_key)
    .bind(&input.fingerprint)
    .bind(&created_at)
    .execute(&mut *transaction)
    .await?;
    let value = sqlx::query_as::<_, PluginRepository>(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM plugin_repositories WHERE id = ?"
    )))
    .bind(&input.id)
    .fetch_one(&mut *transaction)
    .await?;
    let event = repository_event(&value.id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((value, event))
}

/// Switches a repository on or off and renames it; returns whether it exists.
pub(crate) async fn update_plugin_repository(
    connection: &mut SqliteConnection,
    id: &str,
    enabled: Option<bool>,
    name: Option<String>,
) -> Result<(bool, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let result = sqlx::query(
        "UPDATE plugin_repositories \
         SET enabled = COALESCE(?, enabled), name = COALESCE(?, name) WHERE id = ?",
    )
    .bind(enabled)
    .bind(name)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    let event = repository_event(id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((result.rows_affected() > 0, event))
}

/// Removes a third-party repository and its install records. The official one is refused here
/// as well as by the handler: it is compiled in, and removing its row would only bring it back
/// without its replay floor.
pub(crate) async fn delete_plugin_repository(
    connection: &mut SqliteConnection,
    id: &str,
) -> Result<(bool, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    sqlx::query("DELETE FROM plugin_repository_installs WHERE repository_id = ?")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    let result = sqlx::query("DELETE FROM plugin_repositories WHERE id = ? AND kind <> 'official'")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    let event = repository_event(id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((result.rows_affected() > 0, event))
}

/// Records how a refresh ended.
///
/// `MAX(sequence, ?)` for the same reason as the tool manifest's floor: two overlapping
/// refreshes must not let the older of them lower what the newer one raised.
pub(crate) async fn record_plugin_repository_check(
    connection: &mut SqliteConnection,
    id: &str,
    check: RepositoryCheck,
) -> Result<EventEnvelope> {
    let now = Utc::now().to_rfc3339();
    let mut transaction = connection.begin().await?;
    match check {
        RepositoryCheck::Accepted {
            sequence,
            issued_at,
        } => {
            sqlx::query(
                "UPDATE plugin_repositories SET \
                   sequence = MAX(COALESCE(sequence, 0), ?), issued_at = ?, \
                   last_checked_at = ?, last_success_at = ?, last_error = NULL \
                 WHERE id = ?",
            )
            .bind(sequence)
            .bind(issued_at)
            .bind(&now)
            .bind(&now)
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        }
        RepositoryCheck::Failed { code } => {
            sqlx::query(
                "UPDATE plugin_repositories SET last_checked_at = ?, last_error = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(code)
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        }
    }
    let event = repository_event(id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

/// Records a withdrawn signing key; returns whether it was not withdrawn before.
///
/// The first withdrawal stands: a second repository naming the same key changes nothing, and
/// the event is announced only for a new one, so a refresh that merely repeats the list is quiet.
pub(crate) async fn withdraw_plugin_key(
    connection: &mut SqliteConnection,
    input: PluginWithdrawnKey,
) -> Result<(bool, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    let result = sqlx::query(
        "INSERT INTO plugin_withdrawn_keys (fingerprint, key_id, repository_id, withdrawn_at) \
         VALUES (?, ?, ?, ?) ON CONFLICT(fingerprint) DO NOTHING",
    )
    .bind(&input.fingerprint)
    .bind(&input.key_id)
    .bind(&input.repository_id)
    .bind(&input.withdrawn_at)
    .execute(&mut *transaction)
    .await?;
    let newly = result.rows_affected() > 0;
    // A trusted key the index names by id *and* fingerprint stops being trusted in the same
    // write, so the next start does not re-trust what the withdrawal just took away.
    if newly {
        sqlx::query("DELETE FROM plugin_trusted_keys WHERE key_id = ? AND fingerprint = ?")
            .bind(&input.key_id)
            .bind(&input.fingerprint)
            .execute(&mut *transaction)
            .await?;
    }
    let event = EventEnvelope::new(
        EventKind::PluginTrustChanged,
        serde_json::json!({ "resource": "plugin_key_withdrawal", "key_id": input.key_id }),
    );
    if newly {
        insert_event(&mut transaction, &event).await?;
    }
    transaction.commit().await?;
    Ok((newly, event))
}

/// Records that `plugin_id` `version` was installed from `repository_id`.
pub(crate) async fn record_plugin_repository_install(
    connection: &mut SqliteConnection,
    input: PluginRepositoryInstall,
) -> Result<(PluginRepositoryInstall, EventEnvelope)> {
    let mut transaction = connection.begin().await?;
    sqlx::query(
        "INSERT INTO plugin_repository_installs \
           (plugin_id, version, digest, repository_id, installed_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(plugin_id, version) DO UPDATE SET \
           digest = excluded.digest, repository_id = excluded.repository_id, \
           installed_at = excluded.installed_at",
    )
    .bind(&input.plugin_id)
    .bind(&input.version)
    .bind(&input.digest)
    .bind(&input.repository_id)
    .bind(&input.installed_at)
    .execute(&mut *transaction)
    .await?;
    let event = repository_event(&input.repository_id);
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok((input, event))
}

/// What a repository write announces: its id, never its key or address.
///
/// `PluginChanged`, like install and removal: the repository routes are administration-scoped,
/// and an address can carry a token in a deployment that uses one.
fn repository_event(id: &str) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::PluginChanged,
        serde_json::json!({ "resource": "plugin_repository", "repository_id": id }),
    )
}
