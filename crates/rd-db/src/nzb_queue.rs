//! Turns an NZB import into a queue package with one download row per NZB file.

use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{
    DownloadId, DownloadPriority, DownloadState, EventEnvelope, EventKind, NzbImportId, PackageId,
};
use sqlx::{Connection, SqliteConnection};

use crate::{error::StoreError, writer::insert_event};

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
    let (name, category_id, password, state, stored_priority): (
        String,
        Option<String>,
        Option<String>,
        String,
        Option<i64>,
    ) = sqlx::query_as(
        "SELECT name, category_id, password, state, priority FROM nzb_imports WHERE id = ?",
    )
    .bind(import_id.to_string())
    .fetch_optional(&mut *tx)
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
        .fetch_one(&mut *tx)
        .await?;
    if existing > 0 {
        bail!(StoreError::wrong_state("NZB import is already queued"));
    }
    let package_id = PackageId::new();
    // Neither the package nor its folder should carry file extensions (`.nzb`, `.mp4`, …).
    let package_name = rd_files::package_name_from_file_name(&name);
    let package_directory = rd_files::package_directory(destination, &package_name);
    let position: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(position), 0) + 1 FROM packages")
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO packages (id, name, state, destination, category_id, priority, position, kind, nzb_import_id, password, created_at, updated_at) \
         VALUES (?, ?, 'queued', ?, ?, ?, ?, 'usenet', ?, ?, ?, ?)",
    )
    .bind(package_id.to_string())
    .bind(package_name)
    .bind(package_directory.to_string_lossy().as_ref())
    .bind(&category_id)
    .bind(priority.as_i32())
    .bind(position)
    .bind(import_id.to_string())
    .bind(&password)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    let files: Vec<(String, String, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT id, subject, assembly_name, total_bytes, ordinal FROM nzb_files WHERE import_id = ? ORDER BY ordinal",
    )
    .bind(import_id.to_string())
    .fetch_all(&mut *tx)
    .await?;
    if files.is_empty() {
        bail!("NZB import contains no files");
    }
    let initial_state = if start_paused {
        DownloadState::Paused
    } else {
        DownloadState::Queued
    };
    // Resolve every row's name first: postponing the recovery volumes is a decision about the
    // set as a whole, and it may only be taken when the set has a main index to verify with.
    let named: Vec<(String, String, i64, i64)> = files
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
        .collect();
    let postpone = postpone_recovery_volumes(&mut tx, &named).await?;
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
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "UPDATE nzb_imports SET state = 'enqueued', last_error = NULL, updated_at = ? WHERE id = ?",
    )
    .bind(now)
    .bind(import_id.to_string())
    .execute(&mut *tx)
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
        insert_event(&mut tx, event).await?;
    }
    tx.commit().await?;
    Ok((package_id, events))
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
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
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

/// The stable code a finished Usenet row carries while its verdict is still open (RD-108-24).
///
/// It is the marker *and* the explanation: the row sits in `Verifying` with this failure in
/// `last_error_json`, which is what the queue shows the reader and what
/// [`settle_par2_verdicts`] and `recover_interrupted` recognise the row by. A separate column
/// would have said the same thing twice and left the two free to disagree.
pub(crate) const AWAITING_PAR2: &str = "usenet.segments_missing_awaiting_par2";

/// The `LIKE` pattern that finds a row carrying [`AWAITING_PAR2`] in its stored failure.
///
/// The code is quoted in the JSON, so the pattern cannot match a message that merely
/// mentions it.
pub(crate) const AWAITING_PAR2_PATTERN: &str = "%\"usenet.segments_missing_awaiting_par2\"%";

/// States that still say something of this package is on its way.
///
/// The window RD-108-24 defers in: while one of these is left, the set is not settled and no
/// verdict about missing segments can be final. The same list `par2_refill::is_on_the_way`
/// uses, and for the same reason - a row in one of these states is going to arrive without
/// anybody planning for it. `paused` and `skipped` are deliberately not here: neither arrives
/// on its own. A row that is itself waiting for a verdict is `verifying` too and is taken out
/// before this list is consulted.
const ON_THE_WAY: &[&str] = &[
    "queued",
    "resolving",
    "downloading",
    "retry_wait",
    "verifying",
    "repairing",
];

/// Holds back the verdict on a file that finished with holes (RD-108-24).
///
/// The row is complete on disk, with zeros where the missing articles belong - SABnzbd fills
/// the same holes and lets post-processing decide. What cannot be decided yet is whether the
/// set can repair them: in a fully obfuscated post no PAR2 file has declared itself until it
/// has been assembled, so a payload file that happens to finish first would be failed at a
/// moment when the answer is simply not known. The note written here is what the reader sees
/// and what [`settle_par2_verdicts`] picks the row up by.
pub(crate) async fn defer_par2_verdict(
    connection: &mut SqliteConnection,
    id: DownloadId,
    missing: usize,
) -> Result<()> {
    let failure = rd_core::redact_failure(
        rd_core::Failure::coded(
            rd_core::FailureKind::Transient {
                retry_after_seconds: None,
            },
            AWAITING_PAR2,
            format!(
                "{missing} segment(s) were unavailable on every server; the verdict waits until the rest of the set has arrived"
            ),
        )
        .with_param("missing", missing),
    );
    sqlx::query("UPDATE downloads SET last_error_json = ?, updated_at = ? WHERE id = ?")
        .bind(serde_json::to_string(&failure)?)
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut *connection)
        .await?;
    Ok(())
}

/// One row of the package, as the verdict needs to see it.
struct VerdictRow {
    id: String,
    state: String,
    recovery: bool,
    /// The number of missing segments, when this row is waiting for its verdict.
    awaiting: Option<usize>,
}

/// Decides every held-back verdict of a package whose set has settled (RD-108-24).
///
/// Nothing happens while a sibling is still `queued`, `resolving`, `downloading` or waiting
/// for a retry - that is the whole point of the delay. Once none is left, the same question
/// RD-108-23 asks in the runner is asked again, now on a package where every file has its
/// real name and its content has been looked at: does the set carry PAR2 at all? If it does,
/// the row completes and post-processing repairs it; if it does not, it fails with exactly
/// the message it would have failed with immediately.
///
/// Returns the state events of the rows it decided, for the writer to broadcast.
pub(crate) async fn settle_par2_verdicts(
    connection: &mut SqliteConnection,
    package_id: PackageId,
) -> Result<Vec<EventEnvelope>> {
    let rows: Vec<VerdictRow> = sqlx::query_as::<_, (String, String, bool, Option<String>)>(
        "SELECT id, state, recovery, last_error_json FROM downloads WHERE package_id = ?",
    )
    .bind(package_id.to_string())
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(id, state, recovery, last_error)| VerdictRow {
        awaiting: (state == "verifying")
            .then(|| awaiting_missing(last_error.as_deref()))
            .flatten(),
        id,
        state,
        recovery,
    })
    .collect();
    if rows.iter().all(|row| row.awaiting.is_none()) {
        return Ok(Vec::new());
    }
    let on_the_way = rows
        .iter()
        .any(|row| row.awaiting.is_none() && ON_THE_WAY.contains(&row.state.as_str()));
    if on_the_way {
        return Ok(Vec::new());
    }
    let announced = package_subjects_announce_par2(connection, package_id).await?;
    let now = Utc::now();
    let mut events = Vec::new();
    let mut tx = connection.begin().await?;
    for row in rows.iter().filter(|row| row.awaiting.is_some()) {
        let missing = row.awaiting.unwrap_or_default();
        // A PAR2 file with a hole does not vouch for itself (RD-108-23, review round 1).
        let has_par2 = announced
            || rows
                .iter()
                .any(|other| other.id != row.id && other.recovery);
        let event = if has_par2 {
            // The name was written when the file was settled; only the verdict is new.
            sqlx::query(
                "UPDATE downloads SET state = 'completed', last_error_json = NULL, \
                 next_retry_at = NULL, \
                 total_bytes = MAX(COALESCE(total_bytes, 0), committed_bytes), \
                 committed_bytes = MAX(COALESCE(total_bytes, 0), committed_bytes), \
                 updated_at = ? WHERE id = ?",
            )
            .bind(now)
            .bind(&row.id)
            .execute(&mut *tx)
            .await?;
            tracing::info!(
                download_id = %row.id,
                missing,
                "the settled set carries PAR2; the file with holes goes to repair"
            );
            EventEnvelope::new(
                EventKind::DownloadState,
                serde_json::json!({
                    "download_id": row.id,
                    "previous": DownloadState::Verifying,
                    "state": DownloadState::Completed,
                }),
            )
        } else {
            let failure = rd_core::redact_failure(
                rd_core::Failure::coded(
                    rd_core::FailureKind::Permanent,
                    "usenet.segments_missing_no_par2",
                    format!(
                        "{missing} segment(s) were unavailable on every server and the NZB contains no PAR2 repair data"
                    ),
                )
                .with_param("missing", missing),
            );
            sqlx::query(
                "UPDATE downloads SET state = 'failed', last_error_json = ?, \
                 next_retry_at = NULL, updated_at = ? WHERE id = ?",
            )
            .bind(serde_json::to_string(&failure)?)
            .bind(now)
            .bind(&row.id)
            .execute(&mut *tx)
            .await?;
            EventEnvelope::new(
                EventKind::DownloadState,
                serde_json::json!({
                    "download_id": row.id,
                    "previous": DownloadState::Verifying,
                    "state": DownloadState::Failed,
                    "failure": failure,
                }),
            )
        };
        insert_event(&mut tx, &event).await?;
        events.push(event);
    }
    tx.commit().await?;
    Ok(events)
}

/// Packages that still hold a row waiting for its PAR2 verdict (RD-108-24).
pub(crate) async fn packages_awaiting_par2_verdict(
    connection: &mut SqliteConnection,
) -> Result<Vec<PackageId>> {
    sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT package_id FROM downloads \
         WHERE state = 'verifying' AND last_error_json LIKE ?",
    )
    .bind(AWAITING_PAR2_PATTERN)
    .fetch_all(&mut *connection)
    .await?
    .iter()
    .map(|id| id.parse::<PackageId>().map_err(Into::into))
    .collect()
}

/// The number of missing segments a stored failure is holding a verdict open for.
fn awaiting_missing(last_error_json: Option<&str>) -> Option<usize> {
    let failure: rd_core::Failure = serde_json::from_str(last_error_json?).ok()?;
    if failure.code.as_deref() != Some(AWAITING_PAR2) {
        return None;
    }
    Some(
        failure
            .params
            .get("missing")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0),
    )
}

/// Whether any subject of the package's NZB names a PAR2 file.
///
/// The half of the question that survives a file nobody could assemble: a recovery volume
/// whose every segment is gone never gets a row marked `recovery`, but its subject still
/// says what it was.
async fn package_subjects_announce_par2(
    connection: &mut SqliteConnection,
    package_id: PackageId,
) -> Result<bool> {
    let subjects: Vec<String> = sqlx::query_scalar(
        "SELECT nzb_files.subject FROM nzb_files \
         JOIN packages ON packages.nzb_import_id = nzb_files.import_id \
         WHERE packages.id = ?",
    )
    .bind(package_id.to_string())
    .fetch_all(&mut *connection)
    .await?;
    Ok(subjects.iter().any(|subject| {
        let name = rd_collector::subject_file_name(subject).unwrap_or_else(|| subject.clone());
        rd_core::is_recovery_volume(&name)
    }))
}
