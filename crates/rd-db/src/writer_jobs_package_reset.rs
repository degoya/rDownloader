//! Putting a download, or a whole package, back to where the queue first took it: the rows
//! half of a reset. What lies on disk is the scheduler's, and it goes first.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rd_core::{DownloadFile, DownloadId, DownloadState, EventEnvelope, EventKind, PackageState};
use sqlx::Connection;

use crate::{
    error::StoreError,
    writer::{Writer, insert_event},
};

/// Why a reset is refused while a worker holds a row.
const ACTIVE_REFUSAL: &str = "active download must be paused or cancelled before it can be reset";
/// Why a reset is refused for a Usenet file whose package let go of its NZB (DB-15).
const NZB_REFUSAL: &str = "a usenet download whose NZB was dropped cannot be fetched again";

/// Refuses a row that cannot be reset: one a worker holds, or a Usenet file with no articles
/// left to fetch, which queued again could only fail.
pub(super) fn refuse_unresettable(current: &DownloadFile) -> Result<()> {
    if current.state.is_working() {
        bail!(StoreError::wrong_state(ACTIVE_REFUSAL));
    }
    if current.kind == rd_core::DownloadKind::Usenet && current.nzb_file_id.is_none() {
        bail!(StoreError::wrong_state(NZB_REFUSAL));
    }
    Ok(())
}

/// The statements that put one row back to `queued` with everything its transfer produced
/// cleared, inside the caller's transaction. Shared by the reset of one file and of a package.
pub(super) async fn reset_row(
    transaction: &mut sqlx::SqliteConnection,
    id: &str,
    at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query("DELETE FROM chunks WHERE download_id = ?")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    // The refresh counters go back to zero with everything else: a fresh attempt deserves
    // the same budget a new download gets, not the remainder of the last one.
    sqlx::query(
        "UPDATE downloads SET state = 'queued', committed_bytes = 0, total_bytes = NULL, \
         etag = NULL, last_modified = NULL, computed_checksum_algorithm = NULL, \
         computed_checksum_value = NULL, retry_count = 0, next_retry_at = NULL, \
         last_error_json = NULL, resolver_refresh_count = 0, replay_refresh_count = 0, \
         limit_waits = 0, auto_retry_rounds = 0, updated_at = ? WHERE id = ?",
    )
    .bind(at)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    // Usenet keeps its resume state per segment rather than in `chunks`.
    sqlx::query(
        "UPDATE nzb_segments SET state = 'queued', server_attempts = 0, crc32 = NULL \
         WHERE file_id IN \
         (SELECT nzb_file_id FROM downloads WHERE id = ? AND nzb_file_id IS NOT NULL)",
    )
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE nzb_files SET output_path = NULL WHERE id IN \
         (SELECT nzb_file_id FROM downloads WHERE id = ? AND nzb_file_id IS NOT NULL)",
    )
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    Ok(())
}

/// The event a reset row announces itself with.
pub(super) fn reset_event(current: &DownloadFile) -> EventEnvelope {
    EventEnvelope::new(
        EventKind::DownloadState,
        serde_json::json!({
            "download_id": current.id,
            "previous": current.state,
            "state": DownloadState::Queued,
            "reset": true,
        }),
    )
}

impl Writer {
    /// Puts every file of a package back to `queued` and the package as the LinkGrabber
    /// handed it over (RD-1390-07): the files' transfer state as [`Self::reset_download`]
    /// clears it, and the package's own outcome - its post-processing steps, stage, unpack
    /// result and completion time. `forget_outputs` also drops the record of what its unpacks
    /// created, for a reset that deleted those entries.
    ///
    /// What the LinkGrabber decided stays: name, folder, category, priority, password, auth
    /// profile, mirrors and the NZB. One transaction for all of it, so a package is never
    /// half reset; the scheduler deletes the data first and calls this last.
    pub(crate) async fn reset_package(
        &mut self,
        package_id: rd_core::PackageId,
        forget_outputs: bool,
    ) -> Result<Vec<DownloadFile>> {
        let package = package_id.to_string();
        let previous: String = sqlx::query_scalar("SELECT state FROM packages WHERE id = ?")
            .bind(&package)
            .fetch_optional(&mut self.connection)
            .await?
            .context(StoreError::not_found("package not found"))?;
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM downloads WHERE package_id = ? ORDER BY position ASC, created_at ASC",
        )
        .bind(&package)
        .fetch_all(&mut self.connection)
        .await?;
        let mut files = Vec::with_capacity(ids.len());
        for id in &ids {
            let id = id
                .parse::<DownloadId>()
                .map_err(|error| anyhow::anyhow!("stored download id {id}: {error}"))?;
            let current = crate::models::get_download_from_connection(&mut self.connection, id)
                .await?
                .context(StoreError::not_found("download not found"))?;
            // Every row is asked before any is written: one refusal leaves the whole package.
            refuse_unresettable(&current)?;
            files.push(current);
        }
        let mut events: Vec<EventEnvelope> = files.iter().map(reset_event).collect();
        let package_event = EventEnvelope::new(
            EventKind::PackageState,
            serde_json::json!({
                "package_id": package_id,
                "state": PackageState::Queued,
                "previous": previous,
                "stage": null,
                "percent": null,
                "current": null,
                "reset": true,
            }),
        );
        let at = package_event.occurred_at;
        let mut transaction = self.connection.begin().await?;
        for file in &files {
            reset_row(&mut transaction, &file.id.to_string(), at).await?;
        }
        // The pipeline ran over data that is gone; its steps are rebuilt when the package
        // completes next time.
        crate::postprocess_store::delete_for_owner(&mut transaction, &package).await?;
        if forget_outputs {
            crate::package_outputs_store::forget(&mut transaction, &package).await?;
        }
        sqlx::query(
            "UPDATE packages SET state = 'queued', postprocess_stage = NULL, \
             postprocess_percent = NULL, postprocess_current = NULL, extraction_result = NULL, \
             completed_at = NULL, updated_at = ? WHERE id = ?",
        )
        .bind(at)
        .bind(&package)
        .execute(&mut *transaction)
        .await?;
        events.push(package_event);
        for event in &events {
            insert_event(&mut transaction, event).await?;
        }
        transaction.commit().await?;
        for event in events {
            let _ = self.events.send(event);
        }
        let mut reset = Vec::with_capacity(files.len());
        for file in files {
            reset.push(
                crate::models::get_download_from_connection(&mut self.connection, file.id)
                    .await?
                    .context("download disappeared after reset")?,
            );
        }
        Ok(reset)
    }
}
