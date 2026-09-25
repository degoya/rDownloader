//! Package-level queue management: category, priority and manual ordering.

use std::collections::HashMap;

use anyhow::Result;
use chrono::Utc;
use rd_core::{CategoryId, DownloadPackage, DownloadPriority, EventEnvelope, EventKind, PackageId};
use sqlx::{Connection, SqliteConnection, SqlitePool};

use crate::{
    models::{PACKAGE_COLUMNS, PackageRow},
    writer::insert_event,
};

/// A category assignment together with the destination it resolves to for each package.
///
/// The destination has to be per package: every package owns a folder below the category
/// directory, so one shared string would drop the files of every package into the category root
/// side by side.
#[derive(Clone, Debug, Default)]
pub struct CategoryAssignment {
    /// `None` clears the category.
    pub category_id: Option<CategoryId>,
    /// `<category directory>/<package name>` per package id. A package missing from this map
    /// keeps the category and destination it has.
    pub destinations: HashMap<PackageId, String>,
}

/// Optional field changes applied to one or more packages.
#[derive(Clone, Debug, Default)]
pub struct PackageChange {
    /// New category together with the destination resolved for each package.
    pub category: Option<CategoryAssignment>,
    pub priority: Option<DownloadPriority>,
    pub name: Option<String>,
    /// `Some(None)` clears the archive password, `Some(Some(_))` sets it.
    pub password: Option<Option<String>>,
    /// `Some(None)` = inherit from category/default, `Some(Some(level))` = explicit.
    pub postprocess_level: Option<Option<rd_core::PostprocessLevel>>,
    /// `Some(None)` = inherit, `Some(Some(name))` = explicit script.
    pub script: Option<Option<String>>,
}

pub(crate) async fn update_packages(
    connection: &mut SqliteConnection,
    ids: &[PackageId],
    change: &PackageChange,
) -> Result<(Vec<DownloadPackage>, EventEnvelope)> {
    let now = Utc::now();
    let mut transaction = connection.begin().await?;
    for id in ids {
        if let Some(assignment) = &change.category
            && let Some(destination) = assignment.destinations.get(id)
        {
            // `previous_destination` remembers where the data was so the scheduler can sweep the
            // old folder once its last still-running file has been promoted. COALESCE keeps the
            // oldest outstanding one: a second category change before that sweep must not point
            // at an intermediate directory, because an in-flight `.part` never left the first.
            sqlx::query(
                "UPDATE packages SET category_id = ?, \
                 previous_destination = CASE WHEN destination = ? THEN previous_destination \
                     ELSE COALESCE(previous_destination, destination) END, \
                 destination = ?, updated_at = ? WHERE id = ?",
            )
            .bind(assignment.category_id.map(|value| value.to_string()))
            .bind(destination)
            .bind(destination)
            .bind(now)
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
        }
        if let Some(priority) = change.priority {
            sqlx::query("UPDATE packages SET priority = ?, updated_at = ? WHERE id = ?")
                .bind(priority.as_i32())
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *transaction)
                .await?;
        }
        if let Some(name) = &change.name {
            sqlx::query("UPDATE packages SET name = ?, updated_at = ? WHERE id = ?")
                .bind(name)
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *transaction)
                .await?;
        }
        if let Some(password) = &change.password {
            sqlx::query("UPDATE packages SET password = ?, updated_at = ? WHERE id = ?")
                .bind(password)
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *transaction)
                .await?;
        }
        if let Some(level) = change.postprocess_level {
            sqlx::query("UPDATE packages SET postprocess_level = ?, updated_at = ? WHERE id = ?")
                .bind(level.map(crate::writer::level_string))
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *transaction)
                .await?;
        }
        if let Some(script) = &change.script {
            sqlx::query("UPDATE packages SET script = ?, updated_at = ? WHERE id = ?")
                .bind(script)
                .bind(now)
                .bind(id.to_string())
                .execute(&mut *transaction)
                .await?;
        }
    }
    let event = EventEnvelope::new(
        EventKind::PackageState,
        serde_json::json!({ "updated_packages": ids.len() }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    let mut updated = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(row) =
            sqlx::query_as::<_, PackageRow>(&format!("{PACKAGE_COLUMNS} WHERE packages.id = ?"))
                .bind(id.to_string())
                .fetch_optional(&mut *connection)
                .await?
        {
            updated.push(row.try_into()?);
        }
    }
    Ok((updated, event))
}

/// Renames a package and the folder it keeps its files in, in one transaction.
///
/// The row is written first and the disk follows, exactly as a category change does: with
/// `previous_destination` recorded, `Scheduler::relocate_package` can finish — or repeat — the
/// move on the disk, so an interruption between the two leaves the package findable rather than
/// half-moved.
///
/// The prefix rewrite is the part a category change never needed. Two tables hold absolute
/// paths into the package folder, and one of them holds them as an *identity*:
/// `postprocess_steps` is keyed by `(owner_id, kind, source_path)`, and `rd-extract` looks a
/// step up by exactly that. Left pointing at the old folder, a second post-processing run — the
/// `extract/force` button exists to start one on a finished package — would not recognise its
/// own earlier steps and would write a second set of rows beside them. `nzb_files.output_path`
/// is the same story with lower stakes. Both therefore move in the same transaction as
/// `destination`, so no state exists in which half of them point at the new folder.
///
/// Returns `None` when there is no such package.
pub(crate) async fn rename_package_directory(
    connection: &mut SqliteConnection,
    id: PackageId,
    name: &str,
    destination: &str,
) -> Result<(Option<DownloadPackage>, EventEnvelope)> {
    let now = Utc::now();
    let mut transaction = connection.begin().await?;
    let package = id.to_string();
    let Some((previous, import_id)) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT destination, nzb_import_id FROM packages WHERE id = ?",
    )
    .bind(&package)
    .fetch_optional(&mut *transaction)
    .await?
    else {
        transaction.rollback().await?;
        return Ok((
            None,
            EventEnvelope::new(
                EventKind::PackageState,
                serde_json::json!({ "renamed_packages": 0 }),
            ),
        ));
    };

    // COALESCE keeps the oldest outstanding move, for the same reason the category change does:
    // a second rename before the disk caught up must not point the sweep at a folder that never
    // held anything.
    sqlx::query(
        "UPDATE packages SET name = ?, \
         previous_destination = CASE WHEN destination = ? THEN previous_destination \
             ELSE COALESCE(previous_destination, destination) END, \
         destination = ?, updated_at = ? WHERE id = ?",
    )
    .bind(name)
    .bind(destination)
    .bind(destination)
    .bind(now)
    .bind(&package)
    .execute(&mut *transaction)
    .await?;

    // An empty `previous` is not a prefix of anything meaningful — every stored path would
    // match it — so a package that never had a folder simply gets one, and nothing is rewritten.
    if !previous.is_empty() && previous != destination {
        rewrite_stored_paths(
            &mut transaction,
            &package,
            import_id.as_deref(),
            &previous,
            destination,
            now,
        )
        .await?;
    }

    let event = EventEnvelope::new(
        EventKind::PackageState,
        serde_json::json!({ "renamed_packages": 1 }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    let updated =
        sqlx::query_as::<_, PackageRow>(&format!("{PACKAGE_COLUMNS} WHERE packages.id = ?"))
            .bind(&package)
            .fetch_optional(&mut *connection)
            .await?
            .map(TryInto::try_into)
            .transpose()?;
    Ok((updated, event))
}

/// Swaps `previous` for `destination` at the front of every absolute path stored for a package.
///
/// Matching is on whole path components: `<previous>` itself and anything below `<previous>/`,
/// never a sibling folder that merely starts with the same characters — `Show S01` must not
/// drag `Show S01 Extras` along. SQLite's `substr`/`length` count characters rather than bytes,
/// so the offsets are taken in characters too.
///
/// The statements use numbered placeholders (`?1`) rather than plain `?`, because the same five
/// values appear up to six times each. Spelled positionally, the bind list would be twenty-odd
/// calls whose correctness nobody could check by reading them.
async fn rewrite_stored_paths(
    transaction: &mut SqliteConnection,
    owner_id: &str,
    import_id: Option<&str>,
    previous: &str,
    destination: &str,
    now: chrono::DateTime<Utc>,
) -> Result<()> {
    // ?1 = the old folder's length in characters, ?2 = the old folder, ?3 = the first character
    // after it, ?4 = the path separator, ?5 = the new folder.
    let width = i64::try_from(previous.chars().count())?;
    let cut = width + 1;
    let separator = std::path::MAIN_SEPARATOR.to_string();
    /// `<column>` is the old folder itself, or a path below it.
    fn under(column: &str) -> String {
        format!(
            "{column} IS NOT NULL AND substr({column}, 1, ?1) = ?2 \
             AND (length({column}) = ?1 OR substr({column}, ?3, 1) = ?4)"
        )
    }
    /// `<column>` with the old folder at its front swapped for the new one, or left alone.
    fn moved(column: &str) -> String {
        format!(
            "CASE WHEN {} THEN ?5 || substr({column}, ?3) ELSE {column} END",
            under(column)
        )
    }

    sqlx::query(&format!(
        "UPDATE postprocess_steps SET source_path = {source}, output_path = {output}, \
         updated_at = ?6 WHERE owner_id = ?7 AND (({source_under}) OR ({output_under}))",
        source = moved("source_path"),
        output = moved("output_path"),
        source_under = under("source_path"),
        output_under = under("output_path"),
    ))
    .bind(width)
    .bind(previous)
    .bind(cut)
    .bind(&separator)
    .bind(destination)
    .bind(now)
    .bind(owner_id)
    .execute(&mut *transaction)
    .await?;

    // A Usenet package records where each assembled file landed. Those rows belong to the
    // import rather than to the package, so they are reached through it.
    if let Some(import_id) = import_id {
        sqlx::query(&format!(
            "UPDATE nzb_files SET output_path = {output} WHERE import_id = ?6 AND ({under})",
            output = moved("output_path"),
            under = under("output_path"),
        ))
        .bind(width)
        .bind(previous)
        .bind(cut)
        .bind(&separator)
        .bind(destination)
        .bind(import_id)
        .execute(&mut *transaction)
        .await?;
    }
    Ok(())
}

/// Assigns positions 1..n to `ids` in the given order; packages not listed follow
/// afterwards in their previous relative order so positions stay unique.
pub(crate) async fn reorder_packages(
    connection: &mut SqliteConnection,
    ids: &[PackageId],
) -> Result<EventEnvelope> {
    let now = Utc::now();
    let mut transaction = connection.begin().await?;
    let listed: Vec<String> = ids.iter().map(ToString::to_string).collect();
    let remaining: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT id FROM packages ORDER BY position ASC, created_at ASC",
    )
    .fetch_all(&mut *transaction)
    .await?
    .into_iter()
    .filter(|id| !listed.contains(id))
    .collect();
    for (index, id) in listed.iter().chain(remaining.iter()).enumerate() {
        sqlx::query("UPDATE packages SET position = ?, updated_at = ? WHERE id = ?")
            .bind(i64::try_from(index)? + 1)
            .bind(now)
            .bind(id)
            .execute(&mut *transaction)
            .await?;
    }
    let event = EventEnvelope::new(
        EventKind::PackageState,
        serde_json::json!({ "reordered_packages": ids.len() }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

/// File order inside one package (positions 1..n in the given order).
///
/// The caller has already checked that `ids` names exactly this package's files, so the
/// `AND package_id = ?` in the `UPDATE` is a second lock rather than the only one: without it a
/// foreign id would quietly renumber a row of another package.
pub(crate) async fn reorder_downloads(
    connection: &mut SqliteConnection,
    package_id: PackageId,
    ids: &[rd_core::DownloadId],
) -> Result<EventEnvelope> {
    let now = Utc::now();
    let mut transaction = connection.begin().await?;
    let package = package_id.to_string();
    for (index, id) in ids.iter().enumerate() {
        sqlx::query(
            "UPDATE downloads SET position = ?, updated_at = ? WHERE id = ? AND package_id = ?",
        )
        .bind(i64::try_from(index)? + 1)
        .bind(now)
        .bind(id.to_string())
        .bind(&package)
        .execute(&mut *transaction)
        .await?;
    }
    let event = EventEnvelope::new(
        EventKind::PackageState,
        serde_json::json!({ "package_id": package_id, "reordered_downloads": ids.len() }),
    );
    insert_event(&mut transaction, &event).await?;
    transaction.commit().await?;
    Ok(event)
}

/// Where the package's files lived before its last category change, while a sweep of that
/// directory is still outstanding. `None` once there is nothing left to clean up.
pub(crate) async fn previous_destination(
    pool: &SqlitePool,
    id: PackageId,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT previous_destination FROM packages WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(pool)
    .await?
    .flatten())
}

/// Marks the outstanding sweep of a package's former directory as done.
pub(crate) async fn clear_previous_destination(
    connection: &mut SqliteConnection,
    id: PackageId,
) -> Result<()> {
    sqlx::query("UPDATE packages SET previous_destination = NULL WHERE id = ?")
        .bind(id.to_string())
        .execute(connection)
        .await?;
    Ok(())
}

/// Archive password of a package; only the extraction service may read it.
pub(crate) async fn package_password(pool: &SqlitePool, id: PackageId) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, Option<String>>("SELECT password FROM packages WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await?
            .flatten(),
    )
}

/// Carries enricher fields from the link candidates onto the queue rows they became
/// (RD-107-02).
///
/// One transaction for the whole package, because the two halves are one statement about the
/// same enqueue: a package that claims fields none of its files carry would be worse than a
/// package with no fields at all.
///
/// A replace, not an append. Running it twice writes the same rows, which is what makes a
/// retried enqueue harmless. An empty list clears the column.
pub(crate) async fn carry_enrichment(
    connection: &mut SqliteConnection,
    package_id: PackageId,
    package_fields: &[rd_core::EnrichmentField],
    files: &[(rd_core::DownloadId, Vec<rd_core::EnrichmentField>)],
) -> Result<()> {
    let mut transaction = connection.begin().await?;
    sqlx::query("UPDATE packages SET enrichment_json = ? WHERE id = ?")
        .bind(encode_enrichment(package_fields)?)
        .bind(package_id.to_string())
        .execute(&mut *transaction)
        .await?;
    for (id, fields) in files {
        sqlx::query("UPDATE downloads SET enrichment_json = ? WHERE id = ?")
            .bind(encode_enrichment(fields)?)
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

/// An empty list is `NULL` rather than `[]`, so "nothing was enriched" reads the same on a row
/// written before this column existed and on one written after.
pub(crate) fn encode_enrichment(fields: &[rd_core::EnrichmentField]) -> Result<Option<String>> {
    if fields.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string(fields)?))
}

/// Carries enricher fields onto the queue rows one link candidate became (RD-108-15).
///
/// Storing the fields and copying them into the queue are two writer commands with no order
/// between them: an enricher that answers while the auto-queue promotion is already running
/// used to leave its fields on a candidate row the enqueue detaches moments later, so neither
/// the package nor the queue row ever saw them. Carrying them forward here makes both orders
/// end in the same place.
///
/// The row is found by source address, the same way the enqueue matches a candidate to the row
/// it became; the newest one wins, because that is the enqueue this write belongs to. Returns
/// whether a row was found at all.
pub(crate) async fn carry_enrichment_for_source(
    connection: &mut SqliteConnection,
    source_url: &str,
    fields: &[rd_core::EnrichmentField],
) -> Result<bool> {
    if fields.is_empty() {
        return Ok(false);
    }
    let Some((download_id, package_id)) = sqlx::query_as::<_, (String, String)>(
        "SELECT id, package_id FROM downloads WHERE source_url = ? \
         ORDER BY created_at DESC, rowid DESC LIMIT 1",
    )
    .bind(source_url)
    .fetch_optional(&mut *connection)
    .await?
    else {
        return Ok(false);
    };
    sqlx::query("UPDATE downloads SET enrichment_json = ? WHERE id = ?")
        .bind(encode_enrichment(fields)?)
        .bind(&download_id)
        .execute(&mut *connection)
        .await?;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT enrichment_json FROM packages WHERE id = ?")
            .bind(&package_id)
            .fetch_optional(&mut *connection)
            .await?
            .flatten();
    let merged = merge_enrichment(&crate::models::parse_enrichment(stored.as_deref()), fields);
    sqlx::query("UPDATE packages SET enrichment_json = ? WHERE id = ?")
        .bind(encode_enrichment(&merged)?)
        .bind(&package_id)
        .execute(&mut *connection)
        .await?;
    Ok(true)
}

/// The package header keeps one entry per plugin and name, the one that was there first.
///
/// Deduplicated exactly like the union the enqueue builds, so a field that arrives late reads
/// the same as one that was there in time, and the other files' fields are left alone.
fn merge_enrichment(
    existing: &[rd_core::EnrichmentField],
    added: &[rd_core::EnrichmentField],
) -> Vec<rd_core::EnrichmentField> {
    let mut merged = existing.to_vec();
    for field in added {
        if merged
            .iter()
            .any(|kept| kept.plugin_id == field.plugin_id && kept.name == field.name)
        {
            continue;
        }
        merged.push(field.clone());
    }
    merged
}
