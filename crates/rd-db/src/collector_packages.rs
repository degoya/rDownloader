//! LinkGrabber packages: grouping, ordering, moving, online-check claims and enqueue locks.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{
    BatchId, CandidateId, CategoryId, CollectorPackage, CollectorPackageId, DownloadPriority,
    EventEnvelope, EventKind, GrabberEntryKind, GrabberEntryRef, LinkCandidate, LinkCandidateState,
    LinkCheckResult, LinkStatus,
};
use sqlx::{Connection, FromRow, SqliteConnection, SqlitePool};

use crate::{
    collector_store::{CandidateRow, GET_CANDIDATE, enum_string, insert_event},
    error::{StoreError, StoreErrorKind},
    parse_id,
};

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
     password IS NOT NULL AS has_password, password, created_at, postprocess_level, script \
     FROM collector_packages";

pub(crate) async fn list(pool: &SqlitePool) -> Result<Vec<CollectorPackage>> {
    sqlx::query_as::<_, PackageRow>(&format!(
        "{PACKAGE_COLUMNS} WHERE EXISTS (SELECT 1 FROM link_candidates c \
         WHERE c.package_id = collector_packages.id AND c.state != 'enqueued') \
         ORDER BY position ASC, created_at ASC"
    ))
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
    sqlx::query_as::<_, PackageRow>(&format!("{PACKAGE_COLUMNS} WHERE id = ?"))
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
    sqlx::query_as::<_, PackageRow>(&format!("{PACKAGE_COLUMNS} WHERE id = ?"))
        .bind(id.to_string())
        .fetch_optional(connection)
        .await?
        .map(TryInto::try_into)
        .transpose()
}

pub(crate) async fn password(pool: &SqlitePool, id: CollectorPackageId) -> Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT password FROM collector_packages WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .flatten())
}

/// Inserts a package at the end of the queue; returns the new id.
pub(crate) async fn insert(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    batch_id: BatchId,
    name: &str,
    auto_named: bool,
    category_id: Option<CategoryId>,
    priority: DownloadPriority,
    password: Option<&str>,
) -> Result<CollectorPackageId> {
    let id = CollectorPackageId::new();
    let now = Utc::now();
    let position = next_grabber_position(tx).await?;
    sqlx::query(
        "INSERT INTO collector_packages (id, batch_id, name, auto_named, category_id, priority, position, password, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(batch_id.to_string())
    .bind(name)
    .bind(i64::from(auto_named))
    .bind(category_id.map(|value| value.to_string()))
    .bind(priority.as_i32())
    .bind(position)
    .bind(password)
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
    mut query: sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>>,
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
                sqlx::query(&statement).bind(name).bind(now),
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
                sqlx::query(&statement).bind(category.clone()).bind(now),
                &bound,
            )
            .await?;
            let statement =
                format!("UPDATE link_candidates SET category_id = ? WHERE package_id IN ({list})");
            execute_for_ids(&mut tx, sqlx::query(&statement).bind(category), &bound).await?;
        }
        if let Some(priority) = change.priority {
            let statement = format!(
                "UPDATE collector_packages SET priority = ?, updated_at = ? WHERE id IN ({list})"
            );
            execute_for_ids(
                &mut tx,
                sqlx::query(&statement).bind(priority.as_i32()).bind(now),
                &bound,
            )
            .await?;
            let statement =
                format!("UPDATE link_candidates SET priority = ? WHERE package_id IN ({list})");
            execute_for_ids(
                &mut tx,
                sqlx::query(&statement).bind(priority.as_i32()),
                &bound,
            )
            .await?;
        }
        if let Some(password) = &change.password {
            let statement = format!(
                "UPDATE collector_packages SET password = ?, updated_at = ? WHERE id IN ({list})"
            );
            execute_for_ids(
                &mut tx,
                sqlx::query(&statement).bind(password).bind(now),
                &bound,
            )
            .await?;
        }
        if let Some(level) = change.postprocess_level {
            let statement = format!(
                "UPDATE collector_packages SET postprocess_level = ?, updated_at = ? \
                 WHERE id IN ({list})"
            );
            execute_for_ids(
                &mut tx,
                sqlx::query(&statement)
                    .bind(level.map(crate::writer::level_string))
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
                sqlx::query(&statement).bind(script).bind(now),
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
        let mut query = sqlx::query_as::<_, PackageRow>(&statement);
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

/// Positions 1..n for `ids`, unlisted packages follow in their previous order.
pub(crate) async fn reorder(
    connection: &mut SqliteConnection,
    ids: &[CollectorPackageId],
) -> Result<EventEnvelope> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    let listed: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let remaining: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT id FROM collector_packages ORDER BY position ASC, created_at ASC",
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .filter(|id| !listed.contains(id))
    .collect();
    for (index, id) in listed.iter().chain(remaining.iter()).enumerate() {
        sqlx::query("UPDATE collector_packages SET position = ?, updated_at = ? WHERE id = ?")
            .bind(i64::try_from(index)? + 1)
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    let event = collector_event(serde_json::json!({ "reordered_packages": ids.len() }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// An entry as the shared-order query returns it: the kind discriminator and the row id.
///
/// The kind has to travel with the id. Both tables hold UUIDs, so an id alone would be looked up
/// in whichever table happened to be tried first.
fn entry_key(entry: GrabberEntryRef) -> (String, String) {
    let kind = match entry.kind {
        GrabberEntryKind::Collector => "collector",
        GrabberEntryKind::Nzb => "nzb",
    };
    (kind.to_owned(), entry.id.to_string())
}

/// Positions 1..n across both LinkGrabber tables; unlisted entries keep their relative order.
///
/// One transaction covers both tables on purpose. A reorder that renumbered the packages and then
/// failed on the imports would leave a list that is wrong in a way nobody can see: every row still
/// has a position, so the list still renders, and only the sequence is a mixture of two attempts.
///
/// A partial list is accepted. The caller sends the rows it is showing, and the LinkGrabber hides
/// rows routinely - a hoster or state filter, a package whose links are all enqueued, an import
/// that is not yet reviewed - so demanding the complete set would make every drag under a filter
/// either fail or silently drop the rows the person cannot see. What is *not* accepted is a row
/// that does not exist or is claimed under the wrong kind: those write nothing and would report
/// success, which is the same invisible wrong order by another route.
///
/// `after` is where the listed entries are spliced in; `None` means the head of the list. Without
/// it a partial list can only ever describe a prefix, so moving a row at index 700 means sending
/// 701 entries - past the bulk bound the endpoint enforces, and growing with the list. With an
/// anchor a drag sends the row and the row it landed behind, whatever the list's size. The anchor
/// must not itself be listed: it is removed from the sequence along with the other listed entries,
/// so there would be nothing left to splice behind. That case is reported as a missing anchor
/// here and named precisely by the caller, which can see both halves of the request.
pub(crate) async fn reorder_entries(
    connection: &mut SqliteConnection,
    entries: &[GrabberEntryRef],
    after: Option<GrabberEntryRef>,
) -> Result<EventEnvelope> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    let existing: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT kind, id, position FROM ( \
         SELECT 'collector' AS kind, id AS id, position AS position, created_at AS created_at \
         FROM collector_packages \
         UNION ALL SELECT 'nzb', id, position, created_at FROM nzb_imports) \
         ORDER BY position ASC, created_at ASC, id ASC",
    )
    .fetch_all(&mut *tx)
    .await?;
    let order: Vec<(String, String)> = existing
        .iter()
        .map(|(kind, id, _)| (kind.clone(), id.clone()))
        .collect();
    let known: std::collections::HashSet<(String, String)> = order.iter().cloned().collect();
    let mut listed: Vec<(String, String)> = Vec::with_capacity(entries.len());
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for entry in entries {
        let key = entry_key(*entry);
        anyhow::ensure!(
            known.contains(&key),
            StoreError::not_found("LinkGrabber entry not found")
        );
        // A repeated entry would take two of the numbers 1..n and push the last real row off the
        // end of the sequence, so the order the caller asked for is not the one it would get.
        anyhow::ensure!(
            seen.insert(key.clone()),
            StoreError::new(
                StoreErrorKind::Duplicate,
                "LinkGrabber entry listed more than once"
            )
        );
        listed.push(key);
    }
    let remaining: Vec<(String, String)> = order
        .iter()
        .filter(|key| !seen.contains(*key))
        .cloned()
        .collect();
    let sequence: Vec<(String, String)> = match after {
        None => listed.into_iter().chain(remaining).collect(),
        Some(anchor) => {
            let anchor = entry_key(anchor);
            let index = remaining
                .iter()
                .position(|key| *key == anchor)
                .context(StoreError::not_found("LinkGrabber anchor entry not found"))?;
            let (head, tail) = remaining.split_at(index + 1);
            head.iter()
                .cloned()
                .chain(listed)
                .chain(tail.iter().cloned())
                .collect()
        }
    };

    // Only rows that actually move are written. A drag in a list of a few thousand entries
    // renumbers a handful of them; touching the rest would rewrite every `updated_at` and make
    // "when did this change" useless for answering why a row is where it is.
    let previous: std::collections::HashMap<(String, String), i64> = existing
        .into_iter()
        .map(|(kind, id, position)| ((kind, id), position))
        .collect();
    for (index, key) in sequence.iter().enumerate() {
        let position = i64::try_from(index)? + 1;
        if previous.get(key) == Some(&position) {
            continue;
        }
        let statement = if key.0 == "collector" {
            "UPDATE collector_packages SET position = ?, updated_at = ? WHERE id = ?"
        } else {
            "UPDATE nzb_imports SET position = ?, updated_at = ? WHERE id = ?"
        };
        sqlx::query(statement)
            .bind(position)
            .bind(now)
            .bind(&key.1)
            .execute(&mut *tx)
            .await?;
    }
    let event = collector_event(serde_json::json!({ "reordered_entries": entries.len() }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Candidate order inside one package (positions 1..n, unlisted candidates afterwards).
pub(crate) async fn reorder_candidates(
    connection: &mut SqliteConnection,
    package_id: CollectorPackageId,
    ids: &[CandidateId],
) -> Result<EventEnvelope> {
    let mut tx = connection.begin().await?;
    let listed: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let remaining: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT id FROM link_candidates WHERE package_id = ? ORDER BY position ASC, created_at ASC",
    )
    .bind(package_id.to_string())
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .filter(|id| !listed.contains(id))
    .collect();
    for (index, id) in listed.iter().chain(remaining.iter()).enumerate() {
        sqlx::query("UPDATE link_candidates SET position = ? WHERE id = ? AND package_id = ?")
            .bind(i64::try_from(index)? + 1)
            .bind(id)
            .bind(package_id.to_string())
            .execute(&mut *tx)
            .await?;
    }
    let event = collector_event(
        serde_json::json!({ "package_id": package_id, "reordered_candidates": ids.len() }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Moves candidates into an existing or a new package; emptied packages are removed.
pub(crate) async fn move_candidates(
    connection: &mut SqliteConnection,
    ids: &[CandidateId],
    target: MoveTarget,
) -> Result<(CollectorPackage, EventEnvelope)> {
    if ids.is_empty() {
        bail!("no candidates selected");
    }
    let mut tx = connection.begin().await?;
    let first_id = ids[0].to_string();
    let batch_id: String = sqlx::query_scalar("SELECT batch_id FROM link_candidates WHERE id = ?")
        .bind(&first_id)
        .fetch_optional(&mut *tx)
        .await?
        .context(StoreError::not_found("link candidate not found"))?;
    let package_id = match target {
        MoveTarget::Existing(id) => id,
        MoveTarget::New { name } => {
            let (category, priority): (Option<String>, i64) =
                sqlx::query_as("SELECT category_id, priority FROM link_candidates WHERE id = ?")
                    .bind(&first_id)
                    .fetch_one(&mut *tx)
                    .await?;
            insert(
                &mut tx,
                parse_id(&batch_id)?,
                name.trim(),
                false,
                category.as_deref().map(parse_id).transpose()?,
                DownloadPriority::from_i32(i32::try_from(priority).unwrap_or_default()),
                None,
            )
            .await?
        }
    };
    let (category, priority): (Option<String>, i64) =
        sqlx::query_as("SELECT category_id, priority FROM collector_packages WHERE id = ?")
            .bind(package_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .context(StoreError::not_found("target package not found"))?;
    let next_position: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position), 0) FROM link_candidates WHERE package_id = ?",
    )
    .bind(package_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    // Where these links are coming from, read before they leave: a mirror group lives inside
    // one package, so the package losing a member has to be recomputed as well as the one
    // gaining it (RD-110-18).
    let mut touched: Vec<CollectorPackageId> = vec![package_id];
    for id in ids {
        if let Some(previous) = sqlx::query_scalar::<_, Option<String>>(
            "SELECT package_id FROM link_candidates WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .flatten()
        {
            touched.push(parse_id(&previous)?);
        }
    }
    for (offset, id) in ids.iter().enumerate() {
        sqlx::query(
            "UPDATE link_candidates SET package_id = ?, position = ?, category_id = ?, priority = ? \
             WHERE id = ? AND state NOT IN ('resolving', 'enqueued')",
        )
        .bind(package_id.to_string())
        .bind(next_position + i64::try_from(offset)? + 1)
        .bind(&category)
        .bind(priority)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    }
    touched.sort_unstable();
    touched.dedup();
    crate::collector_mirrors::assign(&mut tx, &touched).await?;
    delete_empty_packages(&mut tx).await?;
    let event = collector_event(
        serde_json::json!({ "moved_candidates": ids.len(), "package_id": package_id }),
    );
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let package = sqlx::query_as::<_, PackageRow>(&format!("{PACKAGE_COLUMNS} WHERE id = ?"))
        .bind(package_id.to_string())
        .fetch_one(&mut *connection)
        .await?
        .try_into()?;
    Ok((package, event))
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

/// `(id, file_name, url, package_id, category_id, priority, provider, file_name_declared)`
/// row used while regrouping.
///
/// The provider and whether the name was declared travel with the row because grouping needs
/// them: a container the source named is a release of its own and must not be merged back
/// into a package named after the indexer's host.
type RegroupRow = (
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    i64,
    Option<String>,
    i64,
);

/// One batch's candidates together with the packages grouping proposes for them, carried from
/// the read phase into the write transaction.
type RegroupPlan = (BatchId, Vec<RegroupRow>, Vec<rd_collector::Group>);

/// Re-derives auto-named packages of the given batches from current file names
/// (multipart sets become one package once the online check revealed the real names).
pub(crate) async fn regroup(
    connection: &mut SqliteConnection,
    batch_ids: &[BatchId],
) -> Result<EventEnvelope> {
    // Read and group first, write afterwards. Grouping parses a URL and analyses a name for
    // every candidate of every batch, and it used to run inside the write transaction — the
    // one the serialized writer holds, so the whole database waited on it. Nothing can slip in
    // between the two phases: every mutation goes through the single writer connection this
    // function borrows for the whole command, so the shorter transaction costs no lost update.
    let mut planned: Vec<RegroupPlan> = Vec::new();
    for batch_id in batch_ids {
        let candidates: Vec<RegroupRow> = sqlx::query_as(
            "SELECT c.id, c.file_name, c.url, c.package_id, p.category_id, p.priority, c.provider, c.file_name_declared FROM link_candidates c \
             JOIN collector_packages p ON p.id = c.package_id \
             WHERE c.batch_id = ? AND p.auto_named = 1 AND c.state NOT IN ('enqueued', 'resolving') \
             ORDER BY p.position, c.position",
        )
        .bind(batch_id.to_string())
        .fetch_all(&mut *connection)
        .await?;
        if candidates.is_empty() {
            continue;
        }
        let groups = {
            let hosts: Vec<String> = candidates
                .iter()
                .map(|(_, _, url, ..)| {
                    url::Url::parse(url)
                        .ok()
                        .and_then(|value| value.host_str().map(str::to_owned))
                        .unwrap_or_default()
                })
                .collect();
            let inputs: Vec<rd_collector::GroupInput<'_>> = candidates
                .iter()
                .enumerate()
                .map(|(index, (_, name, _, _, _, _, provider, declared))| {
                    rd_collector::GroupInput {
                        index,
                        file_name: name.as_deref(),
                        host: &hosts[index],
                        standalone: *declared != 0
                            && matches!(
                                provider.as_deref(),
                                Some(rd_core::NZB_PROVIDER | rd_core::TORRENT_PROVIDER)
                            ),
                        // Regrouping an existing batch: the folder a link came from is not
                        // stored on the candidate, so there is nothing to hint with here.
                        // It does not have to be: a package a source named is not auto-named
                        // (RD-120-17) and therefore never reaches this query at all.
                        package_hint: None,
                    }
                })
                .collect();
            rd_collector::group_links(&inputs, None, "Links")
        };
        planned.push((*batch_id, candidates, groups));
    }
    let mut tx = connection.begin().await?;
    let mut changed = 0_usize;
    let mut touched: Vec<CollectorPackageId> = Vec::new();
    for (batch_id, candidates, groups) in &planned {
        for group in groups {
            let members: Vec<&RegroupRow> = group
                .members
                .iter()
                .map(|index| &candidates[*index])
                .collect();
            // Reuse the package most members already live in when its name matches.
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT id FROM collector_packages WHERE batch_id = ? AND auto_named = 1 AND name = ? COLLATE NOCASE",
            )
            .bind(batch_id.to_string())
            .bind(&group.name)
            .fetch_optional(&mut *tx)
            .await?;
            let package_id = match existing {
                Some(id) => id,
                None => {
                    let (category, priority) = (&members[0].4, members[0].5);
                    insert(
                        &mut tx,
                        *batch_id,
                        &group.name,
                        true,
                        category.as_deref().map(parse_id).transpose()?,
                        DownloadPriority::from_i32(i32::try_from(priority).unwrap_or_default()),
                        None,
                    )
                    .await?
                    .to_string()
                }
            };
            for (position, member) in members.iter().enumerate() {
                if member.3 != package_id {
                    changed += 1;
                    touched.push(parse_id(&member.3)?);
                }
                sqlx::query("UPDATE link_candidates SET package_id = ?, position = ? WHERE id = ?")
                    .bind(&package_id)
                    .bind(i64::try_from(position)? + 1)
                    .bind(&member.0)
                    .execute(&mut *tx)
                    .await?;
            }
            touched.push(parse_id(&package_id)?);
        }
    }
    // The regroup after the online check is where sources two and three finally have
    // something to say: until the probe came back the names were the address's last segment
    // and no size was known (RD-110-18). Every package the pass touched is recomputed,
    // including the ones links were taken *out* of, whose groups may now have one member.
    touched.sort_unstable();
    touched.dedup();
    crate::collector_mirrors::assign(&mut tx, &touched).await?;
    delete_empty_packages(&mut tx).await?;
    let event = collector_event(serde_json::json!({ "regrouped_candidates": changed }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Moves checkable candidates into `checking` and returns them with their *pre-claim*
/// state, so callers can tell duplicates apart and restore that state after the check.
pub(crate) async fn claim_for_check(
    connection: &mut SqliteConnection,
    ids: &[CandidateId],
) -> Result<Vec<LinkCandidate>> {
    let mut claimed = Vec::new();
    for id in ids {
        let Some(row) = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
            .bind(id.to_string())
            .fetch_optional(&mut *connection)
            .await?
        else {
            continue;
        };
        // Fresh intakes already sit in `checking`; claiming them is idempotent. Duplicates
        // are probed too so they get a real file name and media metadata instead of the raw
        // URL segment (a duplicated YouTube link would otherwise stay named "watch").
        let result = sqlx::query(
            "UPDATE link_candidates SET state = 'checking' WHERE id = ? \
             AND state IN ('online', 'offline', 'error', 'unsupported', 'checking', 'duplicate')",
        )
        .bind(id.to_string())
        .execute(&mut *connection)
        .await?;
        if result.rows_affected() == 1 {
            claimed.push(row.try_into()?);
        }
    }
    Ok(claimed)
}

/// Stores the outcome of an online check; only rows still in `checking` are touched.
///
/// `was_duplicate` restores the duplicate state after a successful check: the candidate
/// keeps its warning but gains the probed file name and metadata, so it can be
/// re-downloaded deliberately.
///
/// `cached_by` names the provider whose cache answered (RD-130-11). It is written only
/// together with a `cached_at`: a check that did not end in `cached` clears both, so the
/// two columns always describe the same answer.
pub(crate) async fn record_check(
    connection: &mut SqliteConnection,
    id: CandidateId,
    result: Option<LinkCheckResult>,
    error: Option<rd_core::CandidateMessage>,
    was_duplicate: bool,
    cached_by: Option<String>,
) -> Result<EventEnvelope> {
    let media_json = result
        .as_ref()
        .and_then(|result| result.media.as_ref())
        .map(serde_json::to_string)
        .transpose()?;
    let now = Utc::now();
    // Every check writes the column: a cache answer is only as good as the latest check, so
    // one that did not say "cached" must clear what an earlier one said (RD-120-36).
    let cached_at = result
        .as_ref()
        .is_some_and(|result| result.status == LinkStatus::Cached)
        .then_some(now);
    let cached_by = cached_at.is_some().then_some(cached_by).flatten();
    let (state, file_name, size, message) = match result {
        Some(result) => {
            let state = match result.status {
                LinkStatus::Online | LinkStatus::Unknown | LinkStatus::Cached if was_duplicate => {
                    LinkCandidateState::Duplicate
                }
                LinkStatus::Online | LinkStatus::Cached => LinkCandidateState::Online,
                LinkStatus::Offline => LinkCandidateState::Offline,
                LinkStatus::Unknown => LinkCandidateState::Online,
                // Not folded into the duplicate arm above, and that is the point: `Duplicate`
                // is queueable, so a second copy of a page would go right back to being a
                // queued download (RD-110-07).
                LinkStatus::Unresolvable => LinkCandidateState::Unresolvable,
            };
            let file_name = result
                .file_name
                .map(|name| rd_files::sanitize_file_name(&name))
                .filter(|name| !name.is_empty());
            let size = result
                .size
                .map(|value| i64::try_from(value.get()))
                .transpose()?;
            // Only an inconclusive answer leaves a message behind. A link the check
            // confirmed as online or gone needs none, and keeping the previous one would
            // contradict the state next to it. What the caller passes here is the message
            // for *this* situation — it used to be one fallback sentence handed to every
            // candidate of a batch, which is how `Unknown` came to be reported as a missing
            // result (RD-109-43).
            let message = if matches!(
                result.status,
                LinkStatus::Unknown | LinkStatus::Unresolvable
            ) {
                error
            } else {
                None
            };
            (state, file_name, size, message)
        }
        None => (LinkCandidateState::Error, None, None, error),
    };
    let mut tx = connection.begin().await?;
    sqlx::query(
        // A name the check learned — from a content disposition or the hoster itself — is
        // declared by the source, unlike the one intake took from the address.
        "UPDATE link_candidates SET state = ?, file_name = COALESCE(?, file_name), \
         file_name_declared = CASE WHEN ? IS NULL THEN file_name_declared ELSE 1 END, \
         size = COALESCE(?, size), error = ?, error_code = ?, checked_at = ?, cached_at = ?, \
         cached_by = ?, media_json = COALESCE(?, media_json) \
         WHERE id = ? AND state = 'checking'",
    )
    .bind(enum_string(state)?)
    .bind(file_name.clone())
    .bind(file_name)
    .bind(size)
    .bind(message.as_ref().map(|message| message.text.clone()))
    .bind(message.and_then(|message| message.code))
    .bind(now)
    .bind(cached_at)
    .bind(cached_by)
    .bind(media_json)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    let event = collector_event(serde_json::json!({ "candidate_id": id, "state": state }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

/// Records that no check source exists for a candidate (a hoster link without an account
/// and without a free resolver). Marking it `online` instead would claim the link was
/// verified, which is exactly the state the enqueue path must not trust.
///
/// `cached_by` is the provider whose cache holds the file although nothing here can check
/// it (RD-130-11): the state and the message stay, and the cache answer is stamped next to
/// them, so the person sees that another provider has it. `None` clears an earlier stamp.
pub(crate) async fn mark_unsupported(
    connection: &mut SqliteConnection,
    id: CandidateId,
    message: rd_core::CandidateMessage,
    cached_by: Option<String>,
) -> Result<EventEnvelope> {
    let state = LinkCandidateState::Unsupported;
    let now = Utc::now();
    let cached_at = cached_by.is_some().then_some(now);
    let mut tx = connection.begin().await?;
    sqlx::query(
        "UPDATE link_candidates SET state = ?, error = ?, error_code = ?, checked_at = ?, \
         cached_at = ?, cached_by = ? WHERE id = ? AND state = 'checking'",
    )
    .bind(enum_string(state)?)
    .bind(message.text)
    .bind(message.code)
    .bind(now)
    .bind(cached_at)
    .bind(cached_by)
    .bind(id.to_string())
    .execute(&mut *tx)
    .await?;
    let event = collector_event(serde_json::json!({ "candidate_id": id, "state": state }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    Ok(event)
}

pub(crate) async fn set_file_name(
    connection: &mut SqliteConnection,
    id: CandidateId,
    file_name: &str,
) -> Result<(LinkCandidate, EventEnvelope)> {
    let mut tx = connection.begin().await?;
    let result = sqlx::query("UPDATE link_candidates SET file_name = ? WHERE id = ? AND state NOT IN ('resolving', 'enqueued')")
        .bind(file_name).bind(id.to_string()).execute(&mut *tx).await?;
    if result.rows_affected() == 0 {
        bail!(StoreError::not_found("link candidate not found or busy"));
    }
    let event = collector_event(serde_json::json!({ "candidate_id": id, "renamed": true }));
    insert_event(&mut tx, &event).await?;
    tx.commit().await?;
    let candidate = sqlx::query_as::<_, CandidateRow>(GET_CANDIDATE)
        .bind(id.to_string())
        .fetch_one(&mut *connection)
        .await?
        .try_into()?;
    Ok((candidate, event))
}

/// Atomically locks every enqueueable candidate of a package (`resolving`).
///
/// `only` narrows the claim to the links a person could see: a LinkGrabber filter hides links
/// of a package, and "add to the queue" must not send what it hid. The links left out keep
/// their state and their package, which `finish_package_enqueue` then keeps alive for them.
pub(crate) async fn claim_package_for_enqueue(
    connection: &mut SqliteConnection,
    package_id: CollectorPackageId,
    only: Option<&[CandidateId]>,
) -> Result<Vec<(LinkCandidate, LinkCandidateState)>> {
    let mut tx = connection.begin().await?;
    let busy: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM link_candidates WHERE package_id = ? AND state IN ('checking', 'resolving')",
    )
    .bind(package_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if busy > 0 {
        bail!(StoreError::busy("package is being processed"));
    }
    // Built from `LinkCandidateState::ENQUEUEABLE` rather than spelled out, so this list and
    // the one the single-link endpoint applies cannot drift apart again. The states are enum
    // variants, not user input, so there is nothing here to inject.
    let states = LinkCandidateState::ENQUEUEABLE
        .iter()
        .map(|state| Ok(format!("'{}'", crate::collector_store::enum_string(state)?)))
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    let rows = sqlx::query_as::<_, CandidateRow>(&format!(
        "{} WHERE package_id = ? AND state IN ({states}) ORDER BY position, created_at",
        crate::collector_store::CANDIDATE_SELECT
    ))
    .bind(package_id.to_string())
    .fetch_all(&mut *tx)
    .await?;
    let mut claimed = Vec::with_capacity(rows.len());
    for row in rows {
        let candidate: LinkCandidate = row.try_into()?;
        if only.is_some_and(|ids| !ids.contains(&candidate.id)) {
            continue;
        }
        let previous = candidate.state;
        sqlx::query("UPDATE link_candidates SET state = 'resolving' WHERE id = ?")
            .bind(candidate.id.to_string())
            .execute(&mut *tx)
            .await?;
        claimed.push((candidate, previous));
    }
    if claimed.is_empty() {
        bail!(StoreError::new(
            StoreErrorKind::NoEnqueueableLinks,
            "package has no enqueueable links"
        ));
    }
    tx.commit().await?;
    Ok(claimed)
}

/// Releases the enqueue lock: success marks links `enqueued` and detaches the package,
/// failure restores the previous states.
pub(crate) async fn finish_package_enqueue(
    connection: &mut SqliteConnection,
    package_id: CollectorPackageId,
    success: bool,
    restore: &[(CandidateId, LinkCandidateState)],
) -> Result<EventEnvelope> {
    let mut tx = connection.begin().await?;
    if success {
        // Whatever an enricher stored while this enqueue was running (RD-108-15). The claim
        // handed out a snapshot, so a field written after it reached only the candidate row —
        // and this is the last moment that row can still be matched to what it became.
        let late = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT url, enrichment_json FROM link_candidates \
             WHERE package_id = ? AND state = 'resolving'",
        )
        .bind(package_id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        for (url, enrichment) in late {
            let fields = crate::models::parse_enrichment(enrichment.as_deref());
            crate::package_store::carry_enrichment_for_source(&mut tx, &url, &fields).await?;
        }
        sqlx::query("UPDATE link_candidates SET state = 'enqueued', package_id = NULL WHERE package_id = ? AND state = 'resolving'")
            .bind(package_id.to_string()).execute(&mut *tx).await?;
        sqlx::query(
            "DELETE FROM collector_packages WHERE id = ? AND NOT EXISTS \
                     (SELECT 1 FROM link_candidates WHERE package_id = collector_packages.id)",
        )
        .bind(package_id.to_string())
        .execute(&mut *tx)
        .await?;
    } else {
        for (id, state) in restore {
            sqlx::query(
                "UPDATE link_candidates SET state = ? WHERE id = ? AND state = 'resolving'",
            )
            .bind(enum_string(*state)?)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        }
    }
    crate::collector_store::delete_empty_batches(&mut tx).await?;
    let event =
        collector_event(serde_json::json!({ "package_id": package_id, "enqueued": success }));
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
    password: Option<String>,
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
            password: row.password,
            created_at: row.created_at,
            postprocess_level: crate::models::parse_level(row.postprocess_level.as_deref()),
            script: row.script,
        })
    }
}
