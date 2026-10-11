//! The held-back PAR2 verdict of a Usenet file that finished with holes (RD-108-24).

use anyhow::Result;
use chrono::Utc;
use rd_core::{DownloadId, DownloadState, EventEnvelope, EventKind, PackageId};
use sqlx::{Connection, SqliteConnection};

use crate::writer::insert_event;

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
            .then(|| awaiting_missing(&id, last_error.as_deref()))
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
            // The one warning a hole earns (RD-1240-38): the articles are logged at `debug`
            // and the file's count at `info`, because a set with PAR2 repairs them.
            tracing::warn!(
                download_id = %row.id,
                missing,
                "segments missing on every server and the set carries no PAR2; the file stays incomplete"
            );
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
fn awaiting_missing(id: &str, last_error_json: Option<&str>) -> Option<usize> {
    let failure: rd_core::Failure = crate::json_column::lenient(
        serde_json::from_str(last_error_json?),
        "downloads",
        "last_error_json",
        id,
    )?;
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
