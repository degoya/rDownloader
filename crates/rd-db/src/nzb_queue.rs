//! Turns an NZB import into a queue package with one download row per NZB file.

use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{
    DownloadId, DownloadPriority, DownloadState, EventEnvelope, EventKind, NzbImportId, PackageId,
};
use sqlx::{Connection, SqliteConnection};

use crate::{error::StoreError, writer::insert_event};

#[path = "nzb_queue_verdict.rs"]
mod verdict;

pub(crate) use verdict::{
    AWAITING_PAR2, AWAITING_PAR2_PATTERN, defer_par2_verdict, packages_awaiting_par2_verdict,
    settle_par2_verdicts,
};

/// `start_paused` creates every download row of the package in `Paused` instead of `Queued`.
///
/// The package row itself stays `queued`, exactly as the collector path leaves it
/// (`rd_scheduler::enqueue`): the dispatcher only ever picks up `Queued` *downloads*, so a
/// paused NZB is simply never handed to the Usenet runner until somebody starts it.
pub(crate) async fn enqueue_import(
    connection: &mut SqliteConnection,
    import_id: NzbImportId,
    destination: &Path,
    priority: DownloadPriority,
    start_paused: bool,
) -> Result<(PackageId, Vec<EventEnvelope>)> {
    // `destination` is the category directory; the package gets its own folder below it.
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    let (name, category_id, priority) = load_queueable_import(&mut tx, import_id, priority).await?;
    let package_id = insert_import_package(
        &mut tx,
        import_id,
        destination,
        &name,
        category_id,
        priority,
        now,
    )
    .await?;
    let named = named_import_files(&mut tx, import_id).await?;
    let initial_state = if start_paused {
        DownloadState::Paused
    } else {
        DownloadState::Queued
    };
    let postpone = postpone_recovery_volumes(&mut tx, &named).await?;
    insert_import_downloads(
        &mut tx,
        import_id,
        package_id,
        named,
        initial_state,
        postpone,
        now,
    )
    .await?;
    let events = mark_import_enqueued(&mut tx, import_id, package_id, now).await?;
    tx.commit().await?;
    Ok((package_id, events))
}

/// Reads the import's name, category and effective priority, refusing one that cannot be
/// queued (already queued, failed, or already behind a package).
async fn load_queueable_import(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    import_id: NzbImportId,
    priority: DownloadPriority,
) -> Result<(String, Option<String>, DownloadPriority)> {
    // The archive password is not carried here: `Database::enqueue_nzb_import` gives the
    // package a vault entry of its own once the package exists (RD-190-04).
    let (name, category_id, state, stored_priority): (String, Option<String>, String, Option<i64>) =
        sqlx::query_as("SELECT name, category_id, state, priority FROM nzb_imports WHERE id = ?")
            .bind(import_id.to_string())
            .fetch_optional(&mut **tx)
            .await?
            .context(StoreError::not_found("NZB import not found"))?;
    // The priority picked during import wins; the argument stays the fallback for imports
    // created before the column existed.
    let priority =
        stored_priority.map_or(priority, |value| DownloadPriority::from_i32(value as i32));
    if state == "enqueued" {
        bail!(StoreError::wrong_state("NZB import is already queued"));
    }
    // A failed import has no files and no segments, so queueing it would create an empty
    // package that can never finish (RD-108-20). It is a row that says what went wrong, not a
    // candidate; deleting it is what makes room for the file to be imported again.
    if state == "failed" {
        bail!(StoreError::wrong_state(
            "failed NZB import cannot be queued"
        ));
    }
    let existing: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM packages WHERE nzb_import_id = ?")
        .bind(import_id.to_string())
        .fetch_one(&mut **tx)
        .await?;
    if existing > 0 {
        bail!(StoreError::wrong_state("NZB import is already queued"));
    }
    Ok((name, category_id, priority))
}

/// Writes the Usenet package row of the import, at the end of the queue.
async fn insert_import_package(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    import_id: NzbImportId,
    destination: &Path,
    name: &str,
    category_id: Option<String>,
    priority: DownloadPriority,
    now: chrono::DateTime<Utc>,
) -> Result<PackageId> {
    let package_id = PackageId::new();
    // Neither the package nor its folder should carry file extensions (`.nzb`, `.mp4`, …).
    // The name comes from the file or from the adapter that sent it, never from a person, so
    // the package-name rules of the category it lands in apply (RD-1140-05).
    let naming = crate::package_names::naming_for(tx, category_id.as_deref()).await?;
    let package_name =
        rd_files::tidy_package_name(&rd_files::package_name_from_file_name(name), &naming);
    let package_directory = rd_files::package_directory(destination, &package_name);
    let position: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(position), 0) + 1 FROM packages")
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO packages (id, name, state, destination, category_id, priority, position, kind, nzb_import_id, created_at, updated_at) \
         VALUES (?, ?, 'queued', ?, ?, ?, ?, 'usenet', ?, ?, ?)",
    )
    .bind(package_id.to_string())
    .bind(package_name)
    .bind(package_directory.to_string_lossy().as_ref())
    .bind(&category_id)
    .bind(priority.as_i32())
    .bind(position)
    .bind(import_id.to_string())
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(package_id)
}

/// The import's files with the name each queue row gets: `(file id, name, bytes, ordinal)`.
async fn named_import_files(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    import_id: NzbImportId,
) -> Result<Vec<(String, String, i64, i64)>> {
    let files: Vec<(String, String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT id, subject, assembly_name, total_bytes, ordinal FROM nzb_files WHERE import_id = ? ORDER BY ordinal",
    )
    .bind(import_id.to_string())
    .fetch_all(&mut **tx)
    .await?;
    if files.is_empty() {
        bail!("NZB import contains no files");
    }
    // Resolve every row's name first: postponing the recovery volumes is a decision about the
    // set as a whole, and it may only be taken when the set has a main index to verify with.
    Ok(files
        .into_iter()
        .map(|(file_id, subject, assembly_name, total_bytes, ordinal)| {
            let file_name = assembly_name
                .filter(|name| rd_collector::looks_like_file_name(name))
                .or_else(|| rd_collector::subject_file_name(&subject))
                .unwrap_or(subject);
            (
                file_id,
                rd_files::sanitize_file_name(&file_name),
                total_bytes,
                ordinal,
            )
        })
        .collect())
}

/// Writes one download row per NZB file of the package.
async fn insert_import_downloads(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    import_id: NzbImportId,
    package_id: PackageId,
    named: Vec<(String, String, i64, i64)>,
    initial_state: DownloadState,
    postpone: bool,
    now: chrono::DateTime<Utc>,
) -> Result<()> {
    for (file_id, file_name, total_bytes, ordinal) in named {
        // A recovery volume nobody has asked for waits as `Skipped` instead of being fetched
        // with the payload; the PAR2 stage re-queues exactly as many as a repair turns out to
        // need (RD-107-04). The main index is never postponed - it is what answers whether
        // anything is damaged at all.
        let initial_state = if postpone && rd_core::is_par2_volume(&file_name) {
            DownloadState::Skipped
        } else {
            initial_state
        };
        // PAR2 data is marked here rather than recognized later on disk: a volume that expired
        // on the servers has to be distinguishable from a lost payload file while it is still a
        // queue row (RD-107-10). The main index is marked too - RD-107-04 never postpones it,
        // but it can still go missing, and it is still not payload.
        let recovery = rd_core::is_recovery_volume(&file_name);
        sqlx::query(
            "INSERT INTO downloads (id, package_id, source_url, file_name, state, total_bytes, committed_bytes, \
             kind, nzb_file_id, position, recovery, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 0, 'usenet', ?, ?, ?, ?, ?)",
        )
        .bind(rd_core::DownloadId::new().to_string())
        .bind(package_id.to_string())
        .bind(format!("nzb://{import_id}/{file_id}"))
        .bind(&file_name)
        .bind(initial_state.to_string())
        .bind(total_bytes)
        .bind(&file_id)
        .bind(ordinal + 1)
        .bind(recovery)
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Marks the import as queued and records the `usenet.changed` and `package.state` events.
async fn mark_import_enqueued(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    import_id: NzbImportId,
    package_id: PackageId,
    now: chrono::DateTime<Utc>,
) -> Result<Vec<EventEnvelope>> {
    sqlx::query(
        "UPDATE nzb_imports SET state = 'enqueued', last_error = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(import_id.to_string())
    .execute(&mut **tx)
    .await?;
    let events = vec![
        EventEnvelope::new(
            EventKind::UsenetChanged,
            serde_json::json!({ "nzb_import_id": import_id, "state": "enqueued", "package_id": package_id }),
        ),
        EventEnvelope::new(
            EventKind::PackageState,
            serde_json::json!({ "package_id": package_id, "state": DownloadState::Queued }),
        ),
    ];
    for event in &events {
        insert_event(tx, event).await?;
    }
    Ok(events)
}

/// Whether this NZB's `vol` PAR2 volumes should wait instead of downloading with the payload.
///
/// Two conditions, and both are about not taking a decision that cannot be undone. The
/// `enable_all_par` setting must be off - SABnzbd's switch, off by default here as there - and
/// the set must have a main index among its files: postponing every volume of a set whose only
/// recovery data *is* the volumes would leave the package with nothing to verify against and
/// nothing to re-queue from. An unreadable settings blob is read as "off", which is what an
/// installation that never chose gets anyway.
async fn postpone_recovery_volumes(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    named: &[(String, String, i64, i64)],
) -> Result<bool> {
    if enable_all_par(tx).await? {
        return Ok(false);
    }
    let has_index = named
        .iter()
        .any(|(_, file_name, _, _)| rd_core::is_par2_index(file_name));
    let has_volume = named
        .iter()
        .any(|(_, file_name, _, _)| rd_core::is_par2_volume(file_name));
    Ok(has_index && has_volume)
}

/// The `enable_all_par` switch as stored; an unreadable settings blob reads as "off".
///
/// Read inside the import's own transaction rather than through the pool, so the decision and
/// the rows it produces see the same snapshot. The field goes through the shared accessor, so
/// a switch stored with the wrong type is reported instead of quietly reading as "off".
async fn enable_all_par(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<bool> {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT value_json FROM settings WHERE key = 'service.settings'")
            .fetch_optional(&mut **tx)
            .await?;
    Ok(stored
        .as_deref()
        .and_then(|value| {
            crate::json_column::lenient::<serde_json::Value>(
                serde_json::from_str(value),
                "settings",
                "value_json",
                "service.settings",
            )
        })
        .and_then(|blob| crate::service_setting_field_of::<bool>(&blob, "enable_all_par"))
        .unwrap_or(false))
}

/// Settles a Usenet row's name and PAR2 marking once its assembled file is on disk.
///
/// Every PAR2 decision `enqueue_import` takes rests on the name the subject announces, and an
/// obfuscated post announces nothing usable: the row is then named after its raw subject
/// line, `recovery` is false for every file, and nothing is postponed (RD-108-23). This is
/// SABnzbd's `handle_par2` on the queue's side. The row takes the name the file turned out to
/// have, `recovery` is decided again on that name *and* on the file's content, and when the
/// file is the main index of a set, the volumes of that set still waiting in the queue are
/// postponed as `postpone_pars` does - under the same `enable_all_par` switch. A volume that
/// is already downloading, finished or failed is left where it is: postponing is for work
/// that has not started, never a retroactive verdict on work that has.
///
/// Returns the state events of the postponed rows, for the writer to broadcast.
pub(crate) async fn settle_recovery(
    connection: &mut SqliteConnection,
    id: DownloadId,
    file_name: &str,
    content_is_par2: bool,
) -> Result<Vec<EventEnvelope>> {
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    let package_id: String = sqlx::query_scalar("SELECT package_id FROM downloads WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .context(StoreError::not_found("download not found"))?;
    let recovery = rd_core::is_recovery_volume(file_name) || content_is_par2;
    sqlx::query("UPDATE downloads SET file_name = ?, recovery = ?, updated_at = ? WHERE id = ?")
        .bind(file_name)
        .bind(recovery)
        .bind(now)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    let mut events = Vec::new();
    if rd_core::is_par2_index(file_name) && !enable_all_par(&mut tx).await? {
        let waiting: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, file_name, state FROM downloads \
             WHERE package_id = ? AND id != ? AND state IN ('queued', 'paused')",
        )
        .bind(&package_id)
        .bind(id.to_string())
        .fetch_all(&mut *tx)
        .await?;
        for (volume_id, volume_name, state) in waiting {
            if !rd_core::par2_volume_belongs_to(file_name, &volume_name) {
                continue;
            }
            let previous: DownloadState = state.parse()?;
            sqlx::query("UPDATE downloads SET state = 'skipped', updated_at = ? WHERE id = ?")
                .bind(now)
                .bind(&volume_id)
                .execute(&mut *tx)
                .await?;
            let event = EventEnvelope::new(
                EventKind::DownloadState,
                serde_json::json!({
                    "download_id": volume_id,
                    "previous": previous,
                    "state": DownloadState::Skipped,
                }),
            );
            insert_event(&mut tx, &event).await?;
            events.push(event);
        }
    }
    tx.commit().await?;
    Ok(events)
}
