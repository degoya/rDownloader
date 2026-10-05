//! Giving up on a Usenet set that can no longer be repaired (RD-1100-02).
//!
//! The decision is `rd-usenet`'s, which knows the block arithmetic; this is the write that
//! follows it and the rule by which the package then reads as failed. SABnzbd's
//! `fail_hopeless_jobs` does the same: what is downloaded stays where it is, and nothing more
//! of the set is fetched.

use anyhow::{Context, Result};
use chrono::Utc;
use rd_core::{DownloadState, EventEnvelope, EventKind, Failure, PackageId};
use sqlx::{Connection, SqliteConnection};

use crate::{Database, commands::NzbCommand, error::StoreError, writer, writer::insert_event};

/// The stable code every row of a set beyond repair fails with.
pub const USENET_JOB_HOPELESS: &str = "usenet.job_hopeless";

/// The code a finished row with holes carries while its set's verdict is open (RD-108-24),
/// for the runner that has to tell such a row from one still on its way.
pub const USENET_AWAITING_PAR2: &str = crate::nzb_queue::AWAITING_PAR2;

/// The code a PAR2 step carries while it waits for postponed volumes it asked for (RD-107-04),
/// `rd_extract::par2_refill::AWAITING_BLOCKS`; spelled out here because `rd-db` cannot depend
/// on `rd-extract`.
const PAR2_AWAITING_BLOCKS: &str = "postprocess.par2_awaiting_blocks";

/// [`USENET_JOB_HOPELESS`] as it appears in a stored failure: quoted, so a message that merely
/// mentions the code does not match.
const HOPELESS_QUOTED: &str = "\"usenet.job_hopeless\"";

impl Database {
    /// Fails what is left of a Usenet set that cannot be repaired any more (RD-1100-02).
    ///
    /// Every row still waiting for its turn and every row holding its PAR2 verdict open fails
    /// with `failure`; a row that is running is the runner's to stop, and a postponed recovery
    /// volume stays postponed. A PAR2 step waiting for volumes a repair asked for fails with the
    /// same code, so it does not wait under a failed package and a retry runs it afresh. The
    /// package then reads as failed once nothing of it runs.
    pub async fn fail_hopeless_usenet_package(
        &self,
        package_id: PackageId,
        failure: Failure,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| NzbCommand::FailHopelessPackage {
            package_id,
            failure,
            reply,
        })
        .await
    }
}

/// The write behind [`Database::fail_hopeless_usenet_package`], in one transaction.
///
/// Returns the events it recorded, for the writer to broadcast: one state change per row it
/// failed, one step change per PAR2 step it ended, and one `usenet.changed` with `state: "hopeless"` that notifications and
/// automations hang off.
pub(crate) async fn fail_hopeless(
    connection: &mut SqliteConnection,
    package_id: PackageId,
    failure: Failure,
) -> Result<Vec<EventEnvelope>> {
    let failure = rd_core::redact_failure(failure);
    let stored = serde_json::to_string(&failure)?;
    let now = Utc::now();
    let mut tx = connection.begin().await?;
    let import_id: Option<String> =
        sqlx::query_scalar("SELECT nzb_import_id FROM packages WHERE id = ?")
            .bind(package_id.to_string())
            .fetch_optional(&mut *tx)
            .await?
            .context(StoreError::not_found("package not found"))?;
    // `paused` is in here on purpose: a paused file of a set beyond repair is not going to
    // help it either, and leaving it would keep the package from reading as failed.
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, state FROM downloads WHERE package_id = ? \
         AND (state IN ('queued', 'paused', 'retry_wait') \
              OR (state = 'verifying' AND last_error_json LIKE ?))",
    )
    .bind(package_id.to_string())
    .bind(crate::nzb_queue::AWAITING_PAR2_PATTERN)
    .fetch_all(&mut *tx)
    .await?;
    let mut events = Vec::new();
    for (id, state) in rows {
        let previous: DownloadState = state.parse()?;
        sqlx::query(
            "UPDATE downloads SET state = 'failed', last_error_json = ?, next_retry_at = NULL, \
             updated_at = ? WHERE id = ?",
        )
        .bind(&stored)
        .bind(now)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({
                "download_id": id,
                "previous": previous,
                "state": DownloadState::Failed,
                "failure": failure,
            }),
        );
        insert_event(&mut tx, &event).await?;
        events.push(event);
    }
    // The abort can come while post-processing waits for postponed volumes it re-queued
    // (RD-107-04): those volumes have just failed, so the step waiting for them ends too -
    // failed with the set's reason, the convention of a step that did not succeed. A step
    // that is not `completed` runs again on the next pass, so a retry starts it clean.
    let steps: Vec<String> = sqlx::query_scalar(
        "SELECT source_path FROM postprocess_steps \
         WHERE owner_id = ? AND kind = 'par2' AND state = 'queued' AND code = ?",
    )
    .bind(package_id.to_string())
    .bind(PAR2_AWAITING_BLOCKS)
    .fetch_all(&mut *tx)
    .await?;
    let params_json = serde_json::to_string(&failure.params)?;
    for source_path in steps {
        sqlx::query(
            "UPDATE postprocess_steps SET state = 'failed', message = ?, code = ?, \
             params_json = ?, progress_percent = NULL, updated_at = ? \
             WHERE owner_id = ? AND kind = 'par2' AND source_path = ?",
        )
        .bind(&failure.message)
        .bind(USENET_JOB_HOPELESS)
        .bind(&params_json)
        .bind(now)
        .bind(package_id.to_string())
        .bind(&source_path)
        .execute(&mut *tx)
        .await?;
        let event = EventEnvelope::new(
            EventKind::PostprocessProgress,
            serde_json::json!({
                "owner_id": package_id,
                "kind": rd_core::PostprocessKind::Par2,
                "source_path": source_path,
                "state": rd_core::PostprocessState::Failed,
            }),
        );
        insert_event(&mut tx, &event).await?;
        events.push(event);
    }
    let event = EventEnvelope::new(
        EventKind::UsenetChanged,
        serde_json::json!({
            "package_id": package_id,
            "nzb_import_id": import_id,
            "state": "hopeless",
            "code": USENET_JOB_HOPELESS,
            "missing_blocks": failure.params.get("missing_blocks"),
            "available_blocks": failure.params.get("available_blocks"),
        }),
    );
    insert_event(&mut tx, &event).await?;
    events.push(event);
    tx.commit().await?;
    Ok(events)
}

/// Whether a package's rows say the set was given up as beyond repair.
///
/// Nothing of it is left to run - every row finished, failed, was cancelled or is a postponed
/// volume - and a failed row carries [`USENET_JOB_HOPELESS`]. Asked by the package state's
/// derivation, so the package reads as failed whichever of its rows was the last to stop,
/// and again after a restart.
pub(crate) fn gave_up(rows: &[(String, Option<String>)]) -> bool {
    rows.iter().all(|(state, _)| {
        matches!(
            state.as_str(),
            "completed" | "failed" | "cancelled" | "skipped"
        )
    }) && rows.iter().any(|(state, last_error)| {
        state == "failed"
            && last_error
                .as_deref()
                .is_some_and(|failure| failure.contains(HOPELESS_QUOTED))
    })
}
