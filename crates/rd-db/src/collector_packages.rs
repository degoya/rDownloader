//! LinkGrabber packages: grouping, ordering, moving, online-check claims and enqueue locks.

use anyhow::{Result, bail};
use chrono::Utc;
use rd_core::{
    BatchId, CategoryId, CollectorPackage, CollectorPackageId, DownloadPriority, EventEnvelope,
    EventKind,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{collector_store::insert_event, error::StoreError, parse_id};

#[path = "collector_packages_check.rs"]
mod check;
#[path = "collector_packages_enqueue.rs"]
mod enqueue;
#[path = "collector_packages_order.rs"]
mod order;
#[path = "collector_packages_regroup.rs"]
mod regrouping;

pub(crate) use check::{claim_for_check, mark_unsupported, record_check, set_file_name};
pub(crate) use enqueue::{claim_package_for_enqueue, finish_package_enqueue};
pub(crate) use order::{move_candidates, reorder, reorder_candidates, reorder_entries};
pub(crate) use regrouping::regroup;

/// Optional field changes for LinkGrabber packages.
#[derive(Clone, Debug, Default)]
pub struct CollectorPackageChange {
    pub name: Option<String>,
    pub category_id: Option<Option<CategoryId>>,
    pub priority: Option<DownloadPriority>,
    pub password: Option<Option<String>>,
    pub postprocess_level: Option<Option<rd_core::PostprocessLevel>>,
    pub script: Option<Option<String>>,
}

/// Where candidates are moved to.
#[derive(Clone, Debug)]
pub enum MoveTarget {
    Existing(CollectorPackageId),
    New { name: String },
}

const PACKAGE_COLUMNS: &str = "SELECT id, batch_id, name, auto_named, category_id, priority, position, \
     password_ref IS NOT NULL AS has_password, created_at, postprocess_level, script \
     FROM collector_packages";

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<CollectorPackage>> {
    sqlx::query_as::<_, PackageRow>(sqlx::AssertSqlSafe(format!(
        "{PACKAGE_COLUMNS} WHERE EXISTS (SELECT 1 FROM link_candidates c \
         WHERE c.package_id = collector_packages.id AND c.state != 'enqueued') \
         ORDER BY position ASC, created_at ASC"
    )))
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(TryInto::try_into)
    .collect()
}

pub(crate) async fn get(
    pool: &SqlitePool,
    id: CollectorPackageId,
) -> Result<Option<CollectorPackage>> {
    sqlx::query_as::<_, PackageRow>(sqlx::AssertSqlSafe(format!(
        "{PACKAGE_COLUMNS} WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

pub(crate) async fn get_from_connection(
    connection: &mut SqliteConnection,
    id: CollectorPackageId,
) -> Result<Option<CollectorPackage>> {
    sqlx::query_as::<_, PackageRow>(sqlx::AssertSqlSafe(format!(
        "{PACKAGE_COLUMNS} WHERE id = ?"
    )))
    .bind(id.to_string())
    .fetch_optional(connection)
    .await?
    .map(TryInto::try_into)
    .transpose()
}

/// Inserts a package at the end of the queue; returns the new id.
pub(crate) async fn insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    batch_id: BatchId,
    name: &str,
    auto_named: bool,
    category_id: Option<CategoryId>,
    priority: DownloadPriority,
) -> Result<CollectorPackageId> {
    let id = CollectorPackageId::new();
    let now = Utc::now();
    let position = next_grabber_position(tx).await?;
    sqlx::query(
        "INSERT INTO collector_packages (id, batch_id, name, auto_named, category_id, priority, position, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(batch_id.to_string())
    .bind(name)
    .bind(i64::from(auto_named))
    .bind(category_id.map(|value| value.to_string()))
    .bind(priority.as_i32())
    .bind(position)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// Binds one id per placeholder of an `IN (...)` list and runs the statement.
///
/// The list is built from `ids.len()`, never from an id itself, so the `format!` that produces
/// it interpolates a count and nothing a caller supplied.
async fn execute_for_ids<'a>(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    mut query: sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>,
    ids: &[String],
) -> Result<()> {
    for id in ids {
        query = query.bind(id.clone());
    }
    query.execute(&mut **tx).await?;
    Ok(())
}

/// A `?, ?, ...` list of `count` placeholders for an `IN (...)` clause.
fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) async fn update(
    connection: &mut SqliteConnection,
    ids: &[CollectorPackageId],
    change: &CollectorPackageChange,
) -> Result<(Vec<CollectorPackage>, EventEnvelope)> {
    let now = Utc::now();
    let bound: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let list = placeholders(bound.len());
    let mut tx = connection.begin().await?;
    // One statement per changed column rather than up to eight per id: setting the category on
    // 500 selected packages used to cost some 1500 round trips on the single writer connection,
    // and every other write in the process waited behind them.
    if !bound.is_empty() {
        if let Some(name) = &change.name {
            let statement = format!(
                "UPDATE collector_packages SET name = ?, auto_named = 0, updated_at = ? \
                 WHERE id IN ({list})"
            );
            execute_for_ids(
                &mut tx,
                sqlx::query(sqlx::AssertSqlSafe(&*statement))
                    .bind(name)
                    .bind(now),
                &bound,
            )
            .await?;
        }
        if let Some(category) = change.category_id {
            let category = category.map(|value| value.to_string());
            let statement = format!(
                "UPDATE collector_packages SET category_id = ?, updated_at = ? \
                 WHERE id IN ({list})"
            );
            execute_for_ids(
                &mut tx,
                sqlx::query(sqlx::AssertSqlSafe(&*statement))
                    .bind(category.clone())
                    .bind(now),
                &bound,
            )
            .await?;
            let statement =
                format!("UPDATE link_candidates SET category_id = ? WHERE package_id IN ({list})");
            execute_for_ids(
                &mut tx,
                sqlx::query(sqlx::AssertSqlSafe(&*statement)).bind(category),
                &bound,
            )
            .await?;
        }
        if let Some(priority) = change.priority {
            let statement = format!(
                "UPDATE collector_packages SET priority = ?, updated_at = ? WHERE id IN ({list})"
            );
            execute_for_ids(
                &mut tx,
                sqlx::query(sqlx::AssertSqlSafe(&*statement))
                    .bind(priority.as_i32())
                    .bind(now),
                &bound,
            )
            .await?;
            let statement =
                format!("UPDATE link_candidates SET priority = ? WHERE package_id IN ({list})");
            execute_for_ids(
                &mut tx,
                sqlx::query(sqlx::AssertSqlSafe(&*statement)).bind(priority.as_i32()),
                &bound,
            )
            .await?;
        }
        // `change.password` is not a column write: `Database::update_collector_packages`
        // stores it in the vault once this transaction is in (RD-190-04).
        if let Some(level) = change.postprocess_level {
            let statement = format!(
                "UPDATE collector_packages SET postprocess_level = ?, updated_at = ? \
                 WHERE id IN ({list})"
            );
            execute_for_ids(
                &mut tx,
                sqlx::query(sqlx::AssertSqlSafe(&*statement))
                    .bind(level.map(crate::enum_string).transpose()?)
                    .bind(now),
                &bound,
            )
            .await?;
        }
        if let Some(script) = &change.script {
            let statement = format!(
                "UPDATE collector_packages SET script = ?, updated_at = ? WHERE id IN ({list})"
            );
            execute_for_ids(
                &mut tx,
                sqlx::query(sqlx::AssertSqlSafe(&*statement))
                    .bind(script)
                    .bind(now),
                &bound,
            )
            .await?;
        }
    }
    let event = collector_event(serde_json::json!({ "updated_packages": ids.len() }));
    insert_event(&mut tx, &event).await?;
    // Re-read inside the transaction. Outside it the rows could already carry a later writer's
    // change and be reported back as the result of this one, and a read per id is another round
    // trip per package on top of the writes.
    let mut rows = Vec::new();
    if !bound.is_empty() {
        let statement = format!("{PACKAGE_COLUMNS} WHERE id IN ({list})");
        let mut query = sqlx::query_as::<_, PackageRow>(sqlx::AssertSqlSafe(&*statement));
        for id in &bound {
            query = query.bind(id.clone());
        }
        rows = query.fetch_all(&mut *tx).await?;
    }
    tx.commit().await?;
    // `IN` returns the rows in storage order; the caller gets them back in the order it asked
    // for, which is what the loop it replaces delivered. An id nobody has is left out, as before.
    let mut by_id: std::collections::HashMap<String, PackageRow> =
        rows.into_iter().map(|row| (row.id.clone(), row)).collect();
    let mut updated = Vec::with_capacity(bound.len());
    for id in &bound {
        if let Some(row) = by_id.remove(id) {
            updated.push(row.try_into()?);
        }
    }
    Ok((updated, event))
}

/// The next free place in the LinkGrabber's manual order.
///
/// The order is one sequence over `collector_packages` **and** `nzb_imports`, so the maximum has
/// to be taken over both. Taking it per table hands the same number to a new package and a new
/// NZB import, and two rows sharing a position sort against each other by creation time — the
/// row that was dragged somewhere then jumps back the next time the list is read.
pub(crate) async fn next_grabber_position(connection: &mut SqliteConnection) -> Result<i64> {
    let highest: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(highest), 0) FROM ( \
         SELECT COALESCE(MAX(position), 0) AS highest FROM collector_packages \
         UNION ALL SELECT COALESCE(MAX(position), 0) FROM nzb_imports)",
    )
    .fetch_one(connection)
    .await?;
    Ok(highest + 1)
}

pub(crate) async fn delete(
    connection: &mut SqliteConnection,
    id: CollectorPackageId,
) -> Result<EventEnvelope> {
    let busy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM link_candidates WHERE package_id = ? AND state IN ('resolving', 'checking')",
    )
    .bind(id.to_string())
    .fetch_one(&mut *connection)
    .await?;
    if busy > 0 {
        bail!(StoreError::busy("package is being processed"));
    }
    let mut tx = connection.begin().await?;
    sqlx::query("DELETE FROM link_candidates WHERE package_id = ? AND state != 'enqueued'")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    let removed = sqlx::query("DELETE FROM collector_packages WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    if removed.rows_affected() == 0 {
        bail!(StoreError::not_found("package not found"));
    }
    crate::collector_store::delete_empty_batches(&mut tx).await?;
    let event = collector_event(serde_json::json!({ "package_id": id, "removed": true }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

pub(crate) async fn delete_empty_packages(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM collector_packages WHERE NOT EXISTS \
         (SELECT 1 FROM link_candidates WHERE package_id = collector_packages.id AND state != 'enqueued')",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn collector_event(payload: serde_json::Value) -> EventEnvelope {
    EventEnvelope::new(EventKind::CollectorChanged, payload)
}

#[derive(FromRow)]
struct PackageRow {
    id: String,
    batch_id: String,
    name: String,
    auto_named: i64,
    category_id: Option<String>,
    priority: i64,
    position: i64,
    has_password: i64,
    created_at: chrono::DateTime<Utc>,
    postprocess_level: Option<String>,
    script: Option<String>,
}

impl TryFrom<PackageRow> for CollectorPackage {
    type Error = anyhow::Error;

    fn try_from(row: PackageRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            batch_id: parse_id(&row.batch_id)?,
            name: row.name,
            auto_named: row.auto_named != 0,
            category_id: row.category_id.as_deref().map(parse_id).transpose()?,
            priority: DownloadPriority::from_i32(i32::try_from(row.priority).unwrap_or_default()),
            position: row.position,
            has_password: row.has_password != 0,
            // In the vault (RD-190-04); revealed for the answers that show it.
            password: None,
            created_at: row.created_at,
            postprocess_level: crate::models::parse_level(
                row.postprocess_level.as_deref(),
                "collector_packages",
                &row.id,
            ),
            script: row.script,
        })
    }
}
