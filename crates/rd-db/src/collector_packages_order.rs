//! The manual order of packages and NZB imports, the candidate order inside a package, and
//! moving candidates between packages.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{
    CandidateId, CollectorPackage, CollectorPackageId, DownloadPriority, EventEnvelope,
    GrabberEntryKind, GrabberEntryRef,
};
use sqlx::{Connection, SqliteConnection};

use super::{
    MoveTarget, PACKAGE_COLUMNS, PackageRow, collector_event, delete_empty_packages, insert,
};
use crate::{
    collector_store::insert_event,
    error::{StoreError, StoreErrorKind},
    parse_id,
};

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
    // One statement for the whole selection rather than one per link (DB-11).
    let mut touched: Vec<CollectorPackageId> = vec![package_id];
    let previous: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT package_id FROM link_candidates \
         WHERE id IN (SELECT value FROM json_each(?)) AND package_id IS NOT NULL",
    )
    .bind(serde_json::to_string(
        &ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
    )?)
    .fetch_all(&mut *tx)
    .await?;
    for previous in previous {
        touched.push(parse_id(&previous)?);
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
    let package = sqlx::query_as::<_, PackageRow>(sqlx::AssertSqlSafe(format!(
        "{PACKAGE_COLUMNS} WHERE id = ?"
    )))
    .bind(package_id.to_string())
    .fetch_one(&mut *connection)
    .await?
    .try_into()?;
    Ok((package, event))
}
