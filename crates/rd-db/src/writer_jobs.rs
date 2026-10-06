use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{DownloadFile, DownloadId, DownloadState, EventEnvelope, EventKind};
use sqlx::Connection;

use crate::{
    error::StoreError,
    writer::{Writer, insert_event},
};

#[path = "writer_jobs_fields.rs"]
mod fields;
#[path = "writer_jobs_maintenance.rs"]
mod maintenance;
#[path = "writer_jobs_package.rs"]
mod package;

/// Adds a transfer's outcome to the persistent statistics, inside the caller's transaction.
///
/// The provider is the id behind the account the download used, or `direct`; the kind is the
/// row's transport. Nothing a person typed reaches the tables (RD-110-01).
async fn record_transfer_outcome(
    transaction: &mut sqlx::SqliteConnection,
    download: &DownloadFile,
    outcome: crate::stats_store::TransferOutcome,
    now: chrono::DateTime<Utc>,
) -> Result<()> {
    let account_id = download.account_id.map(|id| id.to_string());
    let provider = crate::stats_store::provider_of(transaction, account_id.as_deref()).await?;
    let kind = crate::enum_string(download.kind)?;
    crate::stats_store::record(transaction, &kind, &provider, outcome, now).await
}

/// Drops a package once nothing points at it any more, with everything that hangs off it.
///
/// Shared by [`Writer::delete_download`], which reaches it with the package's last file, and
/// by [`Writer::delete_empty_package`], which reaches it with no file at all. Both have to
/// clear the same three things — the post-processing steps owned by the package, the NZB
/// import it came from, and the row itself — and a second copy of that list is how one of them
/// ends up leaving a dangling import behind.
///
/// The `NOT EXISTS` is the guard, not an optimisation: a package that still has files must
/// survive this call untouched, which is what lets the rollback call it without first working
/// out whether one of its own deletions already took the package along.
async fn remove_package_if_empty(
    transaction: &mut sqlx::SqliteConnection,
    package_id: &str,
) -> Result<bool> {
    // Read before the delete: afterwards the row that names the import is gone.
    let nzb_import: Option<String> =
        sqlx::query_scalar("SELECT nzb_import_id FROM packages WHERE id = ?")
            .bind(package_id)
            .fetch_optional(&mut *transaction)
            .await?
            .flatten();
    let removed = sqlx::query(
        "DELETE FROM packages WHERE id = ? AND NOT EXISTS \
         (SELECT 1 FROM downloads WHERE package_id = packages.id)",
    )
    .bind(package_id)
    .execute(&mut *transaction)
    .await?;
    if removed.rows_affected() == 0 {
        return Ok(false);
    }
    crate::postprocess_store::delete_for_owner(&mut *transaction, package_id).await?;
    if let Some(import_id) = nzb_import {
        sqlx::query("DELETE FROM nzb_imports WHERE id = ?")
            .bind(import_id)
            .execute(&mut *transaction)
            .await?;
    }
    Ok(true)
}

impl Writer {
    /// Discards everything a download has produced so it starts over from zero.
    ///
    /// Deliberately not a lifecycle transition. `Completed -> Queued` and `Seeding -> Queued`
    /// are not legal moves and must not become legal ones just so a reset can exist, so the row
    /// is requeued directly here — the way `recover_interrupted` does after a crash.
    ///
    /// Only what the transfer produced is cleared. What the user chose — the torrent file
    /// selection, the media criteria, the expected checksum, the archive password — is kept,
    /// because the point of a reset is to fetch the same thing again.
    pub(crate) async fn reset_download(&mut self, id: DownloadId) -> Result<DownloadFile> {
        let current = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        if current.state.is_working() {
            bail!(StoreError::wrong_state(
                "active download must be paused or cancelled before it can be reset"
            ));
        }
        // A Usenet file whose package let go of its NZB after it completed
        // (`nzb_store::forget_import_for_package`; the cascade sets `nzb_file_id` to NULL) has
        // no articles left to fetch: queued again it could only fail (DB-15).
        if current.kind == rd_core::DownloadKind::Usenet && current.nzb_file_id.is_none() {
            bail!(StoreError::wrong_state(
                "a usenet download whose NZB was dropped cannot be fetched again"
            ));
        }
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({
                "download_id": id,
                "previous": current.state,
                "state": DownloadState::Queued,
                "reset": true,
            }),
        );
        let mut transaction = self.connection.begin().await?;
        sqlx::query("DELETE FROM chunks WHERE download_id = ?")
            .bind(id.to_string())
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
        .bind(event.occurred_at)
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
        // Usenet keeps its resume state per segment rather than in `chunks`.
        sqlx::query(
            "UPDATE nzb_segments SET state = 'queued', server_attempts = 0, crc32 = NULL \
             WHERE file_id IN \
             (SELECT nzb_file_id FROM downloads WHERE id = ? AND nzb_file_id IS NOT NULL)",
        )
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE nzb_files SET output_path = NULL WHERE id IN \
             (SELECT nzb_file_id FROM downloads WHERE id = ? AND nzb_file_id IS NOT NULL)",
        )
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
        // Post-processing ran over data that is about to be fetched again; its steps say
        // nothing about the package any more and are rebuilt when it completes next time.
        crate::postprocess_store::delete_for_owner(
            &mut transaction,
            &current.package_id.to_string(),
        )
        .await?;
        insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);

        // `refresh_package_state` deliberately never downgrades a finished or post-processing
        // package, so a package that had already been through the pipeline is stepped back
        // explicitly before the ordinary derivation runs.
        let package_state: Option<String> =
            sqlx::query_scalar("SELECT state FROM packages WHERE id = ?")
                .bind(current.package_id.to_string())
                .fetch_optional(&mut self.connection)
                .await?;
        if matches!(
            package_state.as_deref(),
            Some("completed" | "postprocessing")
        ) {
            self.set_package_state(
                current.package_id,
                rd_core::PackageState::Queued,
                None,
                None,
                None,
            )
            .await?;
        }
        self.refresh_package_state(current.package_id).await?;
        crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context("download disappeared after reset")
    }

    pub(crate) async fn delete_download(&mut self, id: DownloadId) -> Result<()> {
        let current = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        if current.state.is_working() {
            bail!(StoreError::wrong_state(
                "active download must be paused or cancelled before removal"
            ));
        }
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({ "download_id": id, "removed": true }),
        );
        let mut transaction = self.connection.begin().await?;
        sqlx::query("DELETE FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *transaction)
            .await?;
        let package_gone =
            remove_package_if_empty(&mut transaction, &current.package_id.to_string()).await?;
        crate::writer::insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        // A removed file leaves the set as surely as one that finished: a sibling held back for
        // the PAR2 verdict (RD-108-24) may have been waiting for exactly this row, and nothing
        // else would ever ask again - it stayed in `Verifying` with no worker behind it. The
        // removal itself is committed, so a failure here is logged rather than reported as one.
        if !package_gone
            && let Err(error) = self.settle_package_after_download(current.package_id).await
        {
            tracing::warn!(
                package_id = %current.package_id,
                %error,
                "the package was not settled after a removal"
            );
        }
        Ok(())
    }

    /// [`Self::delete_download`] for many rows at once (RD-1120-17): every row the store may
    /// remove goes in one transaction, so a batch costs one commit instead of one per row, and
    /// an interruption leaves all of them or none.
    ///
    /// A row that is gone or still working is refused on its own, as the single removal
    /// refuses it, and the others go ahead; the answer holds one entry per id, in order, and a
    /// second mention of an id finds its row gone. A package goes with its last file as it does
    /// there. The removed rows are announced with one `removed` event for the batch
    /// ([`crate::writer::rows_event`]), not one each: 500 at once would overrun the bus.
    pub(crate) async fn delete_downloads(&mut self, ids: &[DownloadId]) -> Result<Vec<Result<()>>> {
        let mut outcomes = Vec::with_capacity(ids.len());
        let mut removed = Vec::new();
        let mut packages = Vec::new();
        let mut transaction = self.connection.begin().await?;
        for &id in ids {
            let Some(current) =
                crate::models::get_download_from_connection(&mut transaction, id).await?
            else {
                outcomes.push(Err(anyhow::anyhow!(StoreError::not_found(
                    "download not found"
                ))));
                continue;
            };
            if current.state.is_working() {
                outcomes.push(Err(anyhow::anyhow!(StoreError::wrong_state(
                    "active download must be paused or cancelled before removal"
                ))));
                continue;
            }
            sqlx::query("DELETE FROM downloads WHERE id = ?")
                .bind(id.to_string())
                .execute(&mut *transaction)
                .await?;
            removed.push(id);
            if !packages.contains(&current.package_id) {
                packages.push(current.package_id);
            }
            outcomes.push(Ok(()));
        }
        let mut remaining = Vec::new();
        for package_id in packages {
            if !remove_package_if_empty(&mut transaction, &package_id.to_string()).await? {
                remaining.push(package_id);
            }
        }
        let event = (!removed.is_empty())
            .then(|| crate::writer::rows_event(&removed, serde_json::json!({ "removed": true })));
        if let Some(event) = &event {
            insert_event(&mut transaction, event).await?;
        }
        transaction.commit().await?;
        if let Some(event) = event {
            let _ = self.events.send(event);
        }
        // As after a single removal: a sibling held back for the PAR2 verdict may have been
        // waiting for one of these rows, and the removal is committed whatever this answers.
        for package_id in remaining {
            if let Err(error) = self.settle_package_after_download(package_id).await {
                tracing::warn!(
                    %package_id,
                    %error,
                    "the package was not settled after a removal"
                );
            }
        }
        Ok(outcomes)
    }

    /// Removes a package that has no files, for a caller that has no download id to offer.
    ///
    /// The rollback of a half-written package needs exactly this: when the *first*
    /// `create_download` fails there is no queue row whose removal would take the package
    /// along, and the empty row used to be left in the queue, where nothing distinguishes it
    /// from a package that is simply still filling up.
    ///
    /// Returns whether a package was removed. A package that still has files is left alone, so
    /// the rollback can call this unconditionally without having to know which of its
    /// `delete_download` calls took the package with it.
    pub(crate) async fn delete_empty_package(&mut self, id: rd_core::PackageId) -> Result<bool> {
        let mut transaction = self.connection.begin().await?;
        if !remove_package_if_empty(&mut transaction, &id.to_string()).await? {
            transaction.rollback().await?;
            return Ok(false);
        }
        let event = EventEnvelope::new(
            EventKind::PackageState,
            serde_json::json!({ "package_id": id, "removed": true }),
        );
        insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        Ok(true)
    }

    pub(crate) async fn record_failure(
        &mut self,
        id: DownloadId,
        failure: rd_core::Failure,
        retry_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<DownloadFile> {
        let current = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        // The single boundary that guarantees no signed URL, cookie or token reaches
        // `downloads.last_error_json` or the `download.state` SSE event: every scheduler and
        // engine path funnels its failure through here. Category and stable code survive,
        // so state mapping and client-side translation are unaffected.
        let failure = rd_core::redact_failure(failure);
        let next = if retry_at.is_some() {
            DownloadState::RetryWait
        } else if matches!(
            failure.category,
            rd_core::FailureKind::AuthRequired
                | rd_core::FailureKind::AccountInvalid
                | rd_core::FailureKind::NeedsCaptcha
                | rd_core::FailureKind::Unsupported
        ) {
            DownloadState::Blocked
        } else {
            DownloadState::Failed
        };
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({ "download_id": id, "state": next, "failure": failure }),
        );
        // Waiting out a limit the hoster imposed is no attempt (RD-191-12): it leaves the retry
        // budget alone and is counted on its own, consecutively — any other outcome starts
        // that count again.
        let limit_wait = retry_at.is_some() && failure.category.is_limit();
        let mut transaction = self.connection.begin().await?;
        sqlx::query(
            "UPDATE downloads SET state = ?, retry_count = retry_count + ?, \
             limit_waits = CASE WHEN ? THEN limit_waits + 1 ELSE 0 END, next_retry_at = ?, \
             last_error_json = ?, updated_at = ? WHERE id = ?",
        )
        .bind(next.to_string())
        .bind(if limit_wait { 0_i64 } else { 1_i64 })
        .bind(limit_wait)
        .bind(retry_at)
        .bind(serde_json::to_string(&failure)?)
        .bind(event.occurred_at)
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
        // The statistics ride in the same transaction as the state (RD-110-01), so a figure
        // there never describes a transfer the queue does not know about.
        let outcome = if retry_at.is_some() {
            crate::stats_store::TransferOutcome::Retried
        } else {
            crate::stats_store::TransferOutcome::Failed
        };
        record_transfer_outcome(&mut transaction, &current, outcome, event.occurred_at).await?;
        // A package whose last file just failed without any having finished never reaches
        // post-processing; its history entry is written here instead (RD-1100-04).
        if next == DownloadState::Failed {
            crate::history_store::record_if_settled_failed(
                &mut transaction,
                &current.package_id.to_string(),
                event.occurred_at,
            )
            .await?;
        }
        insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        let updated = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context("download disappeared after failure")?;
        self.settle_package_after_download(updated.package_id)
            .await?;
        Ok(updated)
    }

    pub(crate) async fn complete_download(
        &mut self,
        id: DownloadId,
        final_name: &str,
        checksum: Option<&rd_core::ExpectedChecksum>,
    ) -> Result<DownloadFile> {
        let current = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        if !current.state.can_transition_to(DownloadState::Completed) {
            bail!("download is not ready to complete");
        }
        let algorithm = checksum
            .map(|value| serde_json::to_string(&value.algorithm))
            .transpose()?
            .map(|value| value.trim_matches('"').to_owned());
        let checksum_value = checksum.map(|value| value.value.as_str());
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({ "download_id": id, "state": DownloadState::Completed }),
        );
        let mut transaction = self.connection.begin().await?;
        // A finished download is 100 % by definition. Runners report progress in throttled
        // samples, so the last sample is usually below the total — and an external tool can
        // finish without a final sample at all, which used to leave a completed download
        // showing a half-full progress bar.
        sqlx::query(
            "UPDATE downloads SET state = 'completed', file_name = ?, computed_checksum_algorithm = ?, \
             computed_checksum_value = ?, next_retry_at = NULL, \
             total_bytes = MAX(COALESCE(total_bytes, 0), committed_bytes), \
             committed_bytes = MAX(COALESCE(total_bytes, 0), committed_bytes), \
             updated_at = ? WHERE id = ?",
        )
        .bind(final_name)
        .bind(algorithm)
        .bind(checksum_value)
        .bind(event.occurred_at)
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
        // What the row now says it moved, after the line above settled the two figures.
        let bytes: i64 = sqlx::query_scalar("SELECT committed_bytes FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_one(&mut *transaction)
            .await?;
        let seconds = (event.occurred_at - current.created_at)
            .num_seconds()
            .max(0);
        record_transfer_outcome(
            &mut transaction,
            &current,
            crate::stats_store::TransferOutcome::Completed {
                bytes: u64::try_from(bytes).unwrap_or_default(),
                seconds: u64::try_from(seconds).unwrap_or_default(),
            },
            event.occurred_at,
        )
        .await?;
        insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        let updated = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context("download disappeared after completion")?;
        self.settle_package_after_download(updated.package_id)
            .await?;
        Ok(updated)
    }
}
