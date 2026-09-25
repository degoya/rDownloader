use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{DownloadFile, DownloadId, DownloadState, EventEnvelope, EventKind};
use sqlx::{Connection, Row};

use crate::{
    error::StoreError,
    writer::{Writer, insert_event},
};

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
    let kind = crate::writer::kind_string(download.kind);
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
        if matches!(
            current.state,
            DownloadState::Resolving
                | DownloadState::Downloading
                | DownloadState::Verifying
                | DownloadState::Repairing
                | DownloadState::Extracting
        ) {
            bail!(StoreError::wrong_state(
                "active download must be paused or cancelled before it can be reset"
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
             updated_at = ? WHERE id = ?",
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
        if matches!(
            current.state,
            DownloadState::Resolving
                | DownloadState::Downloading
                | DownloadState::Verifying
                | DownloadState::Repairing
                | DownloadState::Extracting
        ) {
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
        remove_package_if_empty(&mut transaction, &current.package_id.to_string()).await?;
        crate::writer::insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        Ok(())
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
        let mut transaction = self.connection.begin().await?;
        sqlx::query(
            "UPDATE downloads SET state = ?, retry_count = retry_count + 1, next_retry_at = ?, \
             last_error_json = ?, updated_at = ? WHERE id = ?",
        )
        .bind(next.to_string())
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

    /// The tail of every write that moves one download of a package.
    ///
    /// A row held back for the PAR2 verdict (RD-108-24) is waiting for exactly this moment:
    /// the transition that just happened may have been the last one the set was waiting for.
    /// The verdict is taken before the package state is derived, so the package settles on
    /// the states the verdict leaves behind rather than on the ones it was about to change.
    pub(crate) async fn settle_package_after_download(
        &mut self,
        package_id: rd_core::PackageId,
    ) -> Result<()> {
        let events =
            crate::nzb_queue::settle_par2_verdicts(&mut self.connection, package_id).await?;
        for event in events {
            let _ = self.events.send(event);
        }
        self.refresh_package_state(package_id).await
    }

    /// Derives `packages.state` from its files: any active file → `downloading`; files
    /// still pending → `queued`. Post-processing states are owned by the extraction
    /// service and are left alone until a file becomes active again.
    pub(crate) async fn refresh_package_state(
        &mut self,
        package_id: rd_core::PackageId,
    ) -> Result<()> {
        let Some(current) =
            sqlx::query_scalar::<_, String>("SELECT state FROM packages WHERE id = ?")
                .bind(package_id.to_string())
                .fetch_optional(&mut self.connection)
                .await?
        else {
            return Ok(());
        };
        let states: Vec<String> =
            sqlx::query_scalar("SELECT state FROM downloads WHERE package_id = ?")
                .bind(package_id.to_string())
                .fetch_all(&mut self.connection)
                .await?;
        let active = states.iter().any(|state| {
            matches!(
                state.as_str(),
                "resolving" | "downloading" | "verifying" | "repairing"
            )
        });
        // A skipped mirror never completes by design, so it must not hold the package back;
        // at least one file still has to have finished, or an all-skipped package would
        // announce itself done without a single byte.
        let all_completed = states.iter().any(|state| state == "completed")
            && states
                .iter()
                .all(|state| matches!(state.as_str(), "completed" | "skipped"));
        let next = if active {
            rd_core::PackageState::Downloading
        } else if all_completed || current == "postprocessing" {
            return Ok(());
        } else {
            rd_core::PackageState::Queued
        };
        if next.to_string() == current {
            return Ok(());
        }
        self.set_package_state(package_id, next, None, None, None)
            .await
    }

    /// Writes the package lifecycle state plus live post-processing stage and emits
    /// `package.state`.
    pub(crate) async fn set_package_state(
        &mut self,
        package_id: rd_core::PackageId,
        state: rd_core::PackageState,
        stage: Option<rd_core::PostprocessStage>,
        percent: Option<u8>,
        current: Option<String>,
    ) -> Result<()> {
        let event = EventEnvelope::new(
            EventKind::PackageState,
            serde_json::json!({
                "package_id": package_id,
                "state": state,
                "stage": stage,
                "percent": percent,
                "current": current,
            }),
        );
        let mut transaction = self.connection.begin().await?;
        // Stamped on the way into `Completed` and cleared on the way out, so a package that is
        // restarted starts its removal delay over rather than carrying the old one.
        let completed_at = (state == rd_core::PackageState::Completed).then_some(event.occurred_at);
        sqlx::query(
            "UPDATE packages SET state = ?, postprocess_stage = ?, postprocess_percent = ?, \
             postprocess_current = ?, completed_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(state.to_string())
        .bind(stage.map(|value| value.to_string()))
        .bind(percent.map(i64::from))
        .bind(current)
        .bind(completed_at)
        .bind(event.occurred_at)
        .bind(package_id.to_string())
        .execute(&mut *transaction)
        .await?;
        insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        Ok(())
    }

    /// Persists the unpack outcome of the current post-processing run. Emits no event —
    /// the final `package.state` write follows and triggers the UI refresh.
    pub(crate) async fn set_package_extraction(
        &mut self,
        package_id: rd_core::PackageId,
        result: Option<rd_core::ExtractionResult>,
    ) -> Result<()> {
        sqlx::query("UPDATE packages SET extraction_result = ?, updated_at = ? WHERE id = ?")
            .bind(result.map(|value| value.to_string()))
            .bind(chrono::Utc::now())
            .bind(package_id.to_string())
            .execute(&mut self.connection)
            .await?;
        Ok(())
    }

    /// User-driven rename; refused while the file is active or already finished.
    pub(crate) async fn rename_download(
        &mut self,
        id: DownloadId,
        file_name: &str,
    ) -> Result<DownloadFile> {
        let current = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))?;
        if !matches!(
            current.state,
            DownloadState::Queued
                | DownloadState::Paused
                | DownloadState::RetryWait
                | DownloadState::Failed
                | DownloadState::Blocked
                | DownloadState::Cancelled
        ) {
            bail!(StoreError::wrong_state(
                "download cannot be renamed while active or completed"
            ));
        }
        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({ "download_id": id, "renamed": true }),
        );
        let mut transaction = self.connection.begin().await?;
        // The PAR2 marking follows the name it was taken on (RD-108-23).
        sqlx::query(
            "UPDATE downloads SET file_name = ?, recovery = ?, updated_at = ? WHERE id = ?",
        )
        .bind(file_name)
        .bind(rd_core::is_recovery_volume(file_name))
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut *transaction)
        .await?;
        crate::writer::insert_event(&mut transaction, &event).await?;
        transaction.commit().await?;
        let _ = self.events.send(event);
        crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context(StoreError::not_found("download not found"))
    }

    pub(crate) async fn set_file_name(&mut self, id: DownloadId, file_name: &str) -> Result<()> {
        let result = sqlx::query(
            "UPDATE downloads SET file_name = ?, recovery = ?, updated_at = ? WHERE id = ?",
        )
        .bind(file_name)
        .bind(rd_core::is_recovery_volume(file_name))
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut self.connection)
        .await?;
        if result.rows_affected() != 1 {
            bail!(StoreError::not_found("download not found"));
        }
        Ok(())
    }

    /// Records which vault entry holds this download's transform key (RD-120-11).
    ///
    /// Written once per key rather than per attempt: the reference is part of
    /// `ContentTransform::fingerprint`, so a new one on every attempt would tell every
    /// continuation that its own chunk MACs belonged to somebody else.
    pub(crate) async fn set_transform_key_ref(
        &mut self,
        id: DownloadId,
        reference: Option<String>,
    ) -> Result<()> {
        let result =
            sqlx::query("UPDATE downloads SET transform_key_ref = ?, updated_at = ? WHERE id = ?")
                .bind(reference)
                .bind(Utc::now())
                .bind(id.to_string())
                .execute(&mut self.connection)
                .await?;
        if result.rows_affected() != 1 {
            bail!(StoreError::not_found("download not found"));
        }
        Ok(())
    }

    pub(crate) async fn claim_resolver_refresh(&mut self, id: DownloadId) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE downloads SET resolver_refresh_count = 1, updated_at = ? \
             WHERE id = ? AND resolver_refresh_count = 0",
        )
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut self.connection)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn claim_resolver_pin(
        &mut self,
        id: DownloadId,
        pin: rd_core::ResolverPin,
    ) -> Result<rd_core::ResolverPin> {
        sqlx::query(
            "INSERT INTO download_resolver_pins \
             (download_id, plugin_id, plugin_version, created_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT(download_id) DO NOTHING",
        )
        .bind(id.to_string())
        .bind(pin.plugin_id.to_string())
        .bind(&pin.version)
        .bind(Utc::now())
        .execute(&mut self.connection)
        .await?;
        let row = sqlx::query(
            "SELECT plugin_id, plugin_version FROM download_resolver_pins WHERE download_id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(&mut self.connection)
        .await?
        .context("download resolver pin was not persisted")?;
        Ok(rd_core::ResolverPin {
            plugin_id: crate::parse_id(row.get::<String, _>("plugin_id").as_str())?,
            version: row.get("plugin_version"),
        })
    }

    /// Drops pins naming a resolver version this build can no longer provide.
    ///
    /// A pin keeps a *running* job on one exact resolver version, which is what makes a
    /// mid-download plugin upgrade safe. It was never meant to outlive the version it names:
    /// once that build is gone — an ABI break, a removed third-party plugin — the pin can
    /// never be satisfied again and the job dies with `plugin.pinned_version_missing` on
    /// every retry, forever. The plugin id is stable across versions, so clearing the pin
    /// lets the job resolve through the current build of the same plugin, which is the
    /// outcome the pin was protecting in the first place.
    ///
    /// Runs at startup, before the scheduler starts anything, so no running job loses its pin.
    pub(crate) async fn clear_unsatisfiable_resolver_pins(
        &mut self,
        available: &[(String, String)],
    ) -> Result<u64> {
        let rows = sqlx::query(
            "SELECT download_id, plugin_id, plugin_version FROM download_resolver_pins",
        )
        .fetch_all(&mut self.connection)
        .await?;
        let mut stale = Vec::new();
        for row in rows {
            let plugin_id: String = row.get("plugin_id");
            let version: String = row.get("plugin_version");
            if !available
                .iter()
                .any(|(id, installed)| id == &plugin_id && installed == &version)
            {
                stale.push(row.get::<String, _>("download_id"));
            }
        }
        for download_id in &stale {
            sqlx::query("DELETE FROM download_resolver_pins WHERE download_id = ?")
                .bind(download_id)
                .execute(&mut self.connection)
                .await?;
        }
        Ok(stale.len() as u64)
    }

    pub(crate) async fn recover_interrupted(&mut self) -> Result<u64> {
        let now = Utc::now();
        let mut transaction = self.connection.begin().await?;
        // A row held back for the PAR2 verdict (RD-108-24) is `verifying` and must stay
        // where it is: its file is whole on disk apart from the holes, and requeueing it
        // would fetch the whole file again to arrive at the same open question.
        let result = sqlx::query(
            "UPDATE downloads SET state = 'queued', updated_at = ? \
             WHERE state IN ('resolving', 'downloading', 'repairing') \
             OR (state = 'verifying' \
                 AND (last_error_json IS NULL OR last_error_json NOT LIKE ?))",
        )
        .bind(now)
        .bind(crate::nzb_queue::AWAITING_PAR2_PATTERN)
        .execute(&mut *transaction)
        .await?;
        // Extraction runs on finished files; an interrupted extraction must not re-download.
        sqlx::query(
            "UPDATE downloads SET state = 'completed', updated_at = ? WHERE state = 'extracting'",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query("UPDATE nzb_segments SET state = 'queued' WHERE state = 'downloading'")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("UPDATE postprocess_steps SET state = 'queued' WHERE state = 'running'")
            .execute(&mut *transaction)
            .await?;
        sqlx::query(
            "UPDATE link_candidates SET state = 'online' WHERE state IN ('resolving', 'checking')",
        )
        .execute(&mut *transaction)
        .await?;
        // A package row is always written before its first file — `rd_scheduler::enqueue` and
        // every other creator do it in that order — so a process that stops in that window
        // leaves a package with nothing in it. Nothing in the queue or the interface tells
        // such a row apart from a package that is simply short, so it reads as a finished
        // package that downloaded nothing, and it can never be removed the ordinary way
        // because removal hangs off a file it does not have. `delete_download` already treats
        // a package whose last file is gone as gone; this applies the same rule at the start.
        let empty: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM packages WHERE NOT EXISTS \
             (SELECT 1 FROM downloads WHERE package_id = packages.id)",
        )
        .fetch_all(&mut *transaction)
        .await?;
        for package_id in &empty {
            remove_package_if_empty(&mut transaction, package_id).await?;
        }
        transaction.commit().await?;
        // Whatever the set was waiting for before the restart, it is not running now. A
        // package whose other rows all reached a terminal state before the process stopped
        // gets its verdict here; one whose rows were just requeued keeps waiting for them.
        let waiting =
            crate::nzb_queue::packages_awaiting_par2_verdict(&mut self.connection).await?;
        for package_id in waiting {
            self.settle_package_after_download(package_id).await?;
        }
        Ok(result.rows_affected())
    }

    pub(crate) async fn checkpoint_wal(&mut self) -> Result<()> {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&mut self.connection)
            .await?;
        Ok(())
    }

    pub(crate) async fn set_setting(&mut self, key: &str, value: &serde_json::Value) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value_json, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(serde_json::to_string(value)?)
        .bind(Utc::now())
        .execute(&mut self.connection)
        .await?;
        Ok(())
    }
}
