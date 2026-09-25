//! The writer's own SQL for package, download and chunk rows.
//!
//! The counterpart to [`crate::writer_jobs`], which holds the job-level operations on the same
//! rows. Split out of `writer/mod.rs` so the command arms in [`super::downloads`] and
//! [`super::packages`] stay readable next to the statements they run.

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rd_core::{
    ByteCount, ChunkId, DownloadFile, DownloadId, DownloadState, EventEnvelope, EventKind,
};
use sqlx::{Connection, Row};

use crate::error::StoreError;
use crate::models::PersistedChunk;
use crate::models::{NewDownload, NewPackage};

use super::{PROGRESS_EVENT_INTERVAL, Writer, insert_event, kind_string, level_string};

impl Writer {
    pub(super) async fn create_package(
        &mut self,
        package: NewPackage,
    ) -> Result<rd_core::DownloadPackage> {
        let now = Utc::now();
        let position: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(position), 0) + 1 FROM packages")
                .fetch_one(&mut self.connection)
                .await?;
        sqlx::query(
            "INSERT INTO packages (id, name, state, destination, category_id, priority, position, \
             postprocess_level, script, enrichment_json, created_at, updated_at) \
             VALUES (?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(package.id.to_string())
        .bind(&package.name)
        .bind(&package.destination)
        .bind(package.category_id.map(|id| id.to_string()))
        .bind(package.priority.as_i32())
        .bind(position)
        .bind(package.postprocess_level.map(level_string))
        .bind(&package.script)
        .bind(crate::package_store::encode_enrichment(
            &package.enrichment,
        )?)
        .bind(now)
        .bind(now)
        .execute(&mut self.connection)
        .await?;

        Ok(rd_core::DownloadPackage {
            id: package.id,
            name: package.name,
            state: rd_core::PackageState::Queued,
            created_at: now,
            destination: package.destination,
            category_id: package.category_id,
            priority: package.priority,
            position,
            has_password: false,
            password: None,
            kind: rd_core::DownloadKind::Http,
            nzb_import_id: None,
            completed_at: None,
            postprocess_level: package.postprocess_level,
            script: package.script,
            postprocess: rd_core::PostprocessStatus::default(),
            extraction_result: None,
            // Filled after the enqueue, once the candidates behind the package are known.
            enrichment: Vec::new(),
        })
    }

    pub(super) async fn create_download(&mut self, download: NewDownload) -> Result<DownloadFile> {
        if !matches!(
            download.initial_state,
            // Skipped joins these two because a mirror is held back from the moment it is
            // created: it never passes through the queue on its way to waiting.
            DownloadState::Queued | DownloadState::Paused | DownloadState::Skipped
        ) {
            bail!("new download must start queued, paused or skipped");
        }
        let now = Utc::now();
        let mut tx = self.connection.begin().await?;
        let (algorithm, checksum) = download
            .expected_checksum
            .as_ref()
            .map(|value| {
                let algorithm = serde_json::to_string(&value.algorithm)
                    .map(|encoded| encoded.trim_matches('"').to_owned());
                algorithm.map(|algorithm| (Some(algorithm), Some(value.value.clone())))
            })
            .transpose()?
            .unwrap_or((None, None));

        let (auth_profile_id, auth_profile_pinned) = download.auth_profile.to_columns();
        let position: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM downloads WHERE package_id = ?",
        )
        .bind(download.package_id.to_string())
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO downloads (id, package_id, source_url, file_name, state, total_bytes, \
             committed_bytes, checksum_algorithm, checksum_value, account_id, proxy_profile_id, \
             auth_profile_id, auth_profile_pinned, position, kind, media_json, \
             remote_credential_id, mirror_group, enrichment_json, secret_fragment_ref, \
             created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(download.id.to_string())
        .bind(download.package_id.to_string())
        .bind(download.source.as_str())
        .bind(&download.file_name)
        .bind(download.initial_state.to_string())
        .bind(download.total_bytes.map(|value| value.get() as i64))
        .bind(algorithm)
        .bind(checksum)
        .bind(download.account_id.map(|id| id.to_string()))
        .bind(download.proxy_profile_id.map(|id| id.to_string()))
        .bind(auth_profile_id.map(|id| id.to_string()))
        .bind(auth_profile_pinned)
        .bind(position)
        .bind(kind_string(download.kind))
        .bind(
            download
                .media
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(download.remote_credential_id.map(|id| id.to_string()))
        .bind(download.mirror_group.clone())
        .bind(crate::package_store::encode_enrichment(
            &download.enrichment,
        )?)
        .bind(
            download
                .secret_fragment
                .as_ref()
                .map(|fragment| fragment.reference.clone()),
        )
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        // The download row, its template and the hand-over of the candidate's body
        // reference commit together. A crash between them would leave ciphertext in the
        // vault that nothing points at, or a template whose download never existed.
        if let Some(replay) = &download.replay {
            let template = rd_core::RequestTemplate {
                version: rd_core::REQUEST_TEMPLATE_VERSION,
                request: replay.request.clone(),
                consent: replay.consent.clone(),
            };
            crate::replay_store::insert_request_template(
                &mut tx,
                download.id,
                &template,
                replay.body_ref.as_deref(),
                now,
            )
            .await?;
            if let Some(candidate_id) = replay.candidate_id {
                crate::replay_store::take_candidate_body_ref(&mut tx, candidate_id).await?;
            }
        }
        // Same rule for the vaulted link fragment (RD-110-38): the download row now owns the
        // reference, so the candidate must stop pointing at it in the same transaction --
        // otherwise deleting the candidate later would remove a secret the queue still needs.
        if let Some(fragment) = &download.secret_fragment
            && let Some(candidate_id) = fragment.candidate_id
        {
            crate::collector_store::take_candidate_secret_fragment_ref(&mut tx, candidate_id)
                .await?;
        }

        tx.commit().await?;

        Ok(DownloadFile {
            recording: None,
            id: download.id,
            package_id: download.package_id,
            source: download.source,
            file_name: download.file_name,
            state: download.initial_state,
            total_bytes: download.total_bytes,
            committed_bytes: ByteCount::default(),
            retry_count: 0,
            next_retry_at: None,
            expected_checksum: download.expected_checksum,
            computed_checksum: None,
            last_error: None,
            account_id: download.account_id,
            proxy_profile_id: download.proxy_profile_id,
            remote_credential_id: download.remote_credential_id,
            mirror_group: download.mirror_group.clone(),
            auth_profile: download.auth_profile,
            position,
            kind: download.kind,
            nzb_file_id: None,
            // Recovery data is decided by the NZB when the import is queued
            // (`nzb_queue::enqueue_import`); nothing on this path enqueues one.
            recovery: false,
            media: download.media,
            // Carried over from the candidate after the enqueue, not at insert time: the
            // scheduler builds the rows before anything knows which candidate became which.
            enrichment: Vec::new(),
            created_at: now,
            updated_at: now,
        })
    }

    pub(super) async fn transition_download(
        &mut self,
        id: DownloadId,
        next: DownloadState,
    ) -> Result<DownloadFile> {
        self.transition_download_with_reason(id, next, None).await
    }

    /// Blocks a download and records the cause on the row.
    ///
    /// The cause has to survive the process, because the release paths are not
    /// interchangeable: freeing disk space must restart what the full disk stopped and must
    /// leave a transfer whose validators changed mid-flight exactly where it is.
    pub(super) async fn block_download(
        &mut self,
        id: DownloadId,
        reason: String,
    ) -> Result<DownloadFile> {
        self.transition_download_with_reason(id, DownloadState::Blocked, Some(reason))
            .await
    }

    /// The one transition path. `reason` is written to `block_reason` and cleared by every
    /// transition that does not carry one, so the column never outlives the block it explains.
    async fn transition_download_with_reason(
        &mut self,
        id: DownloadId,
        next: DownloadState,
        reason: Option<String>,
    ) -> Result<DownloadFile> {
        let row = sqlx::query("SELECT state FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut self.connection)
            .await?
            .context(StoreError::not_found("download not found"))?;
        let current: DownloadState = row.get::<String, _>("state").parse()?;
        if !current.can_transition_to(next) {
            bail!("invalid download transition {current} -> {next}");
        }

        let event = EventEnvelope::new(
            EventKind::DownloadState,
            serde_json::json!({ "download_id": id, "previous": current, "state": next }),
        );
        let mut tx = self.connection.begin().await?;
        sqlx::query(
            "UPDATE downloads SET state = ?, \
             last_error_json = CASE WHEN ? = 'queued' THEN NULL ELSE last_error_json END, \
             block_reason = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(next.to_string())
        .bind(next.to_string())
        .bind(reason)
        .bind(event.occurred_at)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        insert_event(&mut tx, &event).await?;
        tx.commit().await?;
        let _ = self.events.send(event);

        let updated = crate::models::get_download_from_connection(&mut self.connection, id)
            .await?
            .context("download disappeared after transition")?;
        // May itself decide a held-back PAR2 verdict, including this row's own when the
        // transition that just happened was the move into `Verifying` (RD-108-24), so the
        // row returned here is the one before that second write.
        self.settle_package_after_download(updated.package_id)
            .await?;
        Ok(updated)
    }

    pub(super) async fn checkpoint_chunk(
        &mut self,
        chunk_id: ChunkId,
        committed_offset: u64,
    ) -> Result<()> {
        let value = i64::try_from(committed_offset).context("chunk offset exceeds SQLite range")?;
        let mut tx = self.connection.begin().await?;
        let row = sqlx::query(
            "SELECT download_id, start_offset, end_offset, committed_offset FROM chunks WHERE id = ?",
        )
        .bind(chunk_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .context("chunk not found")?;
        let previous: i64 = row.get("committed_offset");
        let start: i64 = row.get("start_offset");
        let end: Option<i64> = row.get("end_offset");
        if value < previous || value < start || end.is_some_and(|limit| value > limit) {
            bail!("invalid chunk checkpoint");
        }
        sqlx::query("UPDATE chunks SET committed_offset = ?, updated_at = ? WHERE id = ?")
            .bind(value)
            .bind(Utc::now())
            .bind(chunk_id.to_string())
            .execute(&mut *tx)
            .await?;
        let download_id: String = row.get("download_id");
        sqlx::query(
            "UPDATE downloads SET committed_bytes = (SELECT COALESCE(SUM(committed_offset - start_offset), 0) \
             FROM chunks WHERE download_id = ?), updated_at = ? WHERE id = ?",
        )
        .bind(&download_id)
        .bind(Utc::now())
        .bind(&download_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        self.broadcast_progress(&download_id);
        Ok(())
    }

    /// Progress written by runners that do not use chunk rows (media downloads).
    pub(super) async fn set_download_progress(
        &mut self,
        id: DownloadId,
        committed_bytes: u64,
        total_bytes: Option<u64>,
    ) -> Result<()> {
        let committed = i64::try_from(committed_bytes).context("progress exceeds SQLite range")?;
        let total = total_bytes
            .map(i64::try_from)
            .transpose()
            .context("total exceeds SQLite range")?;
        sqlx::query(
            "UPDATE downloads SET committed_bytes = ?, total_bytes = COALESCE(?, total_bytes), \
             updated_at = ? WHERE id = ?",
        )
        .bind(committed)
        .bind(total)
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut self.connection)
        .await?;
        self.broadcast_progress(&id.to_string());
        Ok(())
    }

    /// Emits at most one `download.progress` event per download and interval.
    pub(super) fn broadcast_progress(&mut self, download_id: &str) {
        let now = std::time::Instant::now();
        let due = self
            .last_progress
            .get(download_id)
            .is_none_or(|last| now.duration_since(*last) >= PROGRESS_EVENT_INTERVAL);
        if !due {
            return;
        }
        self.last_progress.insert(download_id.to_owned(), now);
        if self.last_progress.len() > 1024 {
            self.last_progress
                .retain(|_, last| now.duration_since(*last) < PROGRESS_EVENT_INTERVAL * 10);
        }
        let _ = self.events.send(EventEnvelope::new(
            EventKind::DownloadProgress,
            serde_json::json!({ "download_id": download_id }),
        ));
    }

    /// Writes one finished provider-chunk MAC (RD-103-02, ADR 0011).
    ///
    /// The fingerprint is stored with it and every row of another description is removed in
    /// the same transaction: the condensed value needs all the chunk MACs of *one* stream,
    /// so a half-and-half set would verify nothing and refuse a correct file.
    pub(super) async fn checkpoint_chunk_mac(
        &mut self,
        download_id: DownloadId,
        fingerprint: &str,
        index: u64,
        mac: [u8; 16],
    ) -> Result<()> {
        let index = i64::try_from(index).context("chunk index exceeds SQLite range")?;
        let mut tx = self.connection.begin().await?;
        sqlx::query("DELETE FROM transform_chunk_macs WHERE download_id = ? AND fingerprint <> ?")
            .bind(download_id.to_string())
            .bind(fingerprint)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO transform_chunk_macs (download_id, chunk_index, mac, fingerprint, updated_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(download_id, chunk_index) DO UPDATE SET \
             mac = excluded.mac, fingerprint = excluded.fingerprint, updated_at = excluded.updated_at",
        )
        .bind(download_id.to_string())
        .bind(index)
        .bind(mac.to_vec())
        .bind(fingerprint)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn prepare_transfer(
        &mut self,
        id: DownloadId,
        total_bytes: Option<u64>,
        etag: Option<String>,
        last_modified: Option<String>,
        chunks: Vec<PersistedChunk>,
    ) -> Result<()> {
        let total = total_bytes
            .map(|value| i64::try_from(value).context("file size exceeds SQLite range"))
            .transpose()?;
        let mut tx = self.connection.begin().await?;
        let committed =
            sqlx::query_scalar::<_, i64>("SELECT committed_bytes FROM downloads WHERE id = ?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await?
                .context(StoreError::not_found("download not found"))?;
        if committed != 0 {
            bail!("cannot replace chunk plan after committed progress");
        }
        sqlx::query("DELETE FROM chunks WHERE download_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        // A new chunk plan is a new run of this file from zero. Chunk MACs accumulated for
        // the old one describe bytes nobody is going to write again.
        sqlx::query("DELETE FROM transform_chunk_macs WHERE download_id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
        for chunk in chunks {
            sqlx::query(
                "INSERT INTO chunks (id, download_id, start_offset, end_offset, committed_offset, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(chunk.id.to_string())
            .bind(id.to_string())
            .bind(i64::try_from(chunk.start)?)
            .bind(chunk.end.map(i64::try_from).transpose()?)
            .bind(i64::try_from(chunk.committed)?)
            .bind(Utc::now())
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "UPDATE downloads SET total_bytes = ?, etag = ?, last_modified = ?, updated_at = ? WHERE id = ?",
        )
        .bind(total)
        .bind(etag)
        .bind(last_modified)
        .bind(Utc::now())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
