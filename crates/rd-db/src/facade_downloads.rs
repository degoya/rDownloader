//! Database facade for packages and queue rows: creation, state changes, chunk checkpoints,
//! torrent state and the reads the queue is built from.

use anyhow::Result;
use rd_core::{DownloadFile, DownloadId, DownloadPackage};

use crate::{
    Database, NewDownload, NewPackage, PersistedChunk, TransferMetadata,
    commands::{ConfigCommand, DownloadsCommand},
    models, replay_store, torrent_store, writer,
};

impl Database {
    /// Creates an empty package.
    pub async fn create_package(&self, package: NewPackage) -> Result<DownloadPackage> {
        writer::request(&self.writer, |reply| DownloadsCommand::CreatePackage {
            package,
            reply,
        })
        .await
    }

    /// Adds a file to a package and creates its initial chunk.
    pub async fn create_download(&self, download: NewDownload) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| DownloadsCommand::CreateDownload {
            download,
            sources: None,
            reply,
        })
        .await
    }

    /// Announces the rows one enqueue created with a single `download.state` event carrying
    /// `created: true` (RD-1120-17); see `Writer::announce_created` for the payload. Nothing
    /// happens for an empty list.
    pub async fn announce_created_downloads(
        &self,
        package_id: rd_core::PackageId,
        ids: Vec<DownloadId>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| DownloadsCommand::AnnounceCreated {
            package_id,
            ids,
            reply,
        })
        .await
    }

    /// Changes a file state and commits a matching event atomically.
    pub async fn transition_download(
        &self,
        id: DownloadId,
        next: rd_core::DownloadState,
    ) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| DownloadsCommand::TransitionDownload {
            id,
            next,
            reply,
        })
        .await
    }

    /// Queues a row an enqueue wrote paused, unless it was touched after `created_at`; answers
    /// the row as it is afterwards. See `Writer::join_queue` for why it is no transition.
    pub async fn join_queue(
        &self,
        id: DownloadId,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| DownloadsCommand::JoinQueue {
            id,
            created_at,
            reply,
        })
        .await
    }

    /// Blocks a file and records why, so the release path can tell the causes apart.
    ///
    /// `Blocked` is one state for several unrelated causes and the releases are not
    /// interchangeable — freeing disk space must not restart a transfer whose validators
    /// changed mid-flight. The vocabulary of reasons belongs to the caller; this layer only
    /// stores the string and hands it back through [`Self::downloads_blocked_by`].
    pub async fn block_download(&self, id: DownloadId, reason: &str) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| DownloadsCommand::BlockDownload {
            id,
            reason: reason.to_owned(),
            reply,
        })
        .await
    }

    /// Ids of the blocked files that were blocked for `reason`.
    ///
    /// Ids only: the caller already holds the rows it cares about, and this read exists to
    /// narrow a release down to one cause, not to load the table a second time.
    pub async fn downloads_blocked_by(&self, reason: &str) -> Result<Vec<DownloadId>> {
        models::downloads_blocked_by(&self.readers, reason).await
    }

    /// Removes an inactive queue entry and its now-empty package metadata.
    pub async fn delete_download(&self, id: DownloadId) -> Result<()> {
        let references = self.vaulted_references(id).await;
        writer::request(&self.writer, |reply| DownloadsCommand::DeleteDownload {
            id,
            reply,
        })
        .await?;
        self.forget_secrets(references).await;
        // The package went with its last file, and its archive password with it (RD-190-04).
        self.sweep_archive_passwords().await;
        Ok(())
    }

    /// [`Self::delete_download`] for many rows in one writer transaction (RD-1120-17).
    ///
    /// One answer per id, in order: a row that is gone or still working is refused on its own
    /// and the others are removed. Only the vaulted material of a removed row is forgotten, and
    /// the archive passwords are swept once for the whole batch.
    ///
    /// # Errors
    ///
    /// When the transaction itself fails; then no row of the batch was removed.
    pub async fn delete_downloads(&self, ids: Vec<DownloadId>) -> Result<Vec<Result<()>>> {
        let mut references = Vec::with_capacity(ids.len());
        for &id in &ids {
            references.push(self.vaulted_references(id).await);
        }
        let outcomes = writer::request(&self.writer, |reply| DownloadsCommand::DeleteDownloads {
            ids,
            reply,
        })
        .await?;
        let released = references
            .into_iter()
            .zip(&outcomes)
            .filter(|(_, outcome)| outcome.is_ok())
            .flat_map(|(references, _)| references)
            .collect();
        self.forget_secrets(released).await;
        self.sweep_archive_passwords().await;
        Ok(outcomes)
    }

    /// The vault references a download row holds, read before the row goes: afterwards nothing
    /// names them any more.
    async fn vaulted_references(&self, id: DownloadId) -> Vec<String> {
        // The second owner of a vaulted link fragment (RD-110-38). Read before the delete for
        // the same reason the candidate's is: the reference is a column of the row going away.
        let orphaned = self.download_secret_fragment_ref(id).await.unwrap_or(None);
        // The third owner of vaulted material on this row (RD-120-11): the transform key.
        // Read before the delete for the same reason, and forgotten with the fragment.
        let key_reference = self.download_transform_key_ref(id).await.unwrap_or(None);
        // The fourth: the captured body of a POST replay. Its template row cascades with the
        // download (migration 0027), so after the delete nothing names it any more.
        let body_reference = replay_store::template_body_ref(&self.readers, id)
            .await
            .unwrap_or(None);
        orphaned
            .into_iter()
            .chain(key_reference)
            .chain(body_reference)
            .collect()
    }

    /// The reference a download row holds, without opening the vault.
    async fn download_secret_fragment_ref(&self, id: DownloadId) -> Result<Option<String>> {
        use sqlx::Row;

        let row = sqlx::query("SELECT secret_fragment_ref FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.readers)
            .await?;
        Ok(row.and_then(|row| {
            row.try_get::<Option<String>, _>("secret_fragment_ref")
                .ok()
                .flatten()
        }))
    }

    /// Removes a package that has no files at all; returns whether one was removed.
    ///
    /// The package-level counterpart of [`Self::delete_download`], which can only drop a
    /// package as a side effect of removing its last file. A half-written package whose very
    /// first file failed has no such file, and the empty row it left behind reads in the queue
    /// exactly like a package that downloaded nothing. A package that still has files is not
    /// touched, so a rollback may call this unconditionally.
    pub async fn delete_empty_package(&self, id: rd_core::PackageId) -> Result<bool> {
        let removed = writer::request(&self.writer, |reply| DownloadsCommand::DeleteEmptyPackage {
            id,
            reply,
        })
        .await?;
        if removed {
            self.sweep_archive_passwords().await;
        }
        Ok(removed)
    }

    /// Persists a post-sync checkpoint for a chunk.
    /// Progress of a runner-driven file (media): bytes so far and, when known, the total.
    pub async fn set_download_progress(
        &self,
        id: DownloadId,
        committed_bytes: u64,
        total_bytes: Option<u64>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            DownloadsCommand::SetDownloadProgress {
                id,
                committed_bytes,
                total_bytes,
                reply,
            }
        })
        .await
    }

    /// Replaces the seeding override of one category; `None` clears it.
    pub async fn set_category_seeding_policy(
        &self,
        id: rd_core::CategoryId,
        policy: Option<rd_core::SeedingPolicyOverride>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            ConfigCommand::SetCategorySeedingPolicy { id, policy, reply }
        })
        .await
    }

    /// Replaces the torrent state of one link candidate (file tree, plan, metadata).
    pub async fn set_candidate_torrent_state(
        &self,
        id: rd_core::CandidateId,
        state: rd_core::TorrentCandidateState,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            DownloadsCommand::SetCandidateTorrentState {
                id,
                state: Box::new(state),
                reply,
            }
        })
        .await
    }

    /// Replaces the torrent state of one queue row (plan, trackers, seeding, accounting).
    pub async fn set_download_torrent_state(
        &self,
        id: DownloadId,
        state: rd_core::TorrentJobState,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            DownloadsCommand::SetDownloadTorrentState {
                id,
                state: Box::new(state),
                reply,
            }
        })
        .await
    }

    /// Torrent state of one link candidate.
    pub async fn candidate_torrent_state(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<rd_core::TorrentCandidateState>> {
        let mut connection = self.readers.acquire().await?;
        torrent_store::candidate_state(&mut connection, id).await
    }

    /// Torrent state of one queue row.
    pub async fn download_torrent_state(
        &self,
        id: DownloadId,
    ) -> Result<Option<rd_core::TorrentJobState>> {
        let mut connection = self.readers.acquire().await?;
        torrent_store::download_state(&mut connection, id).await
    }

    /// Torrent state of every queue row that has one, for restart recovery.
    pub async fn all_download_torrent_states(
        &self,
    ) -> Result<Vec<(DownloadId, rd_core::TorrentJobState)>> {
        let mut connection = self.readers.acquire().await?;
        torrent_store::all_download_states(&mut connection).await
    }

    pub async fn checkpoint_chunk(
        &self,
        chunk_id: rd_core::ChunkId,
        committed_offset: u64,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| DownloadsCommand::CheckpointChunk {
            chunk_id,
            committed_offset,
            reply,
        })
        .await
    }

    /// Records one finished provider-chunk MAC of a transformed stream (RD-103-02).
    pub async fn checkpoint_chunk_mac(
        &self,
        download_id: DownloadId,
        fingerprint: String,
        index: u64,
        mac: [u8; 16],
    ) -> Result<()> {
        writer::request(&self.writer, |reply| DownloadsCommand::CheckpointChunkMac {
            download_id,
            fingerprint,
            index,
            mac,
            reply,
        })
        .await
    }

    /// What a previous attempt at this file finished, for the transform to adopt.
    ///
    /// Empty for every ordinary download, which has no transform and therefore no MACs.
    pub async fn transform_checkpoint(
        &self,
        download_id: DownloadId,
    ) -> Result<(Option<String>, Vec<(usize, [u8; 16])>)> {
        let mut connection = self.readers.acquire().await?;
        models::transform_checkpoint(&mut connection, download_id).await
    }

    /// Stores validators and a newly planned set of chunks before downloading starts.
    pub async fn prepare_transfer(
        &self,
        id: DownloadId,
        total_bytes: Option<u64>,
        etag: Option<String>,
        last_modified: Option<String>,
        chunks: Vec<PersistedChunk>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| DownloadsCommand::PrepareTransfer {
            id,
            total_bytes,
            etag,
            last_modified,
            chunks,
            reply,
        })
        .await
    }

    /// Loads crash-safe range and validator metadata.
    pub async fn load_transfer(&self, id: DownloadId) -> Result<TransferMetadata> {
        models::load_transfer(&self.readers, id).await
    }

    /// Persists a scheduler failure, retry time and resulting state.
    pub async fn record_failure(
        &self,
        id: DownloadId,
        failure: rd_core::Failure,
        retry_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| DownloadsCommand::RecordFailure {
            id,
            failure,
            retry_at,
            reply,
        })
        .await
    }

    /// Marks a verified file complete and stores its optional generated checksum.
    pub async fn complete_download(
        &self,
        id: DownloadId,
        final_name: String,
        checksum: Option<rd_core::ExpectedChecksum>,
    ) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| DownloadsCommand::CompleteDownload {
            id,
            final_name,
            checksum,
            reply,
        })
        .await
    }

    /// Persists a collision-resolved filename before file IO starts.
    pub async fn set_download_file_name(&self, id: DownloadId, file_name: String) -> Result<()> {
        writer::request(&self.writer, |reply| DownloadsCommand::SetFileName {
            id,
            file_name,
            reply,
        })
        .await
    }

    /// Renames a file that is not active or finished; returns the updated row.
    pub async fn rename_download(&self, id: DownloadId, file_name: String) -> Result<DownloadFile> {
        writer::request(&self.writer, |reply| DownloadsCommand::RenameDownload {
            id,
            file_name,
            reply,
        })
        .await
    }

    /// Returns packages in queue order, without their archive passwords: the scheduler reads
    /// this on every pass, and only `has_password` is in the row (RD-190-04).
    pub async fn list_packages(&self) -> Result<Vec<DownloadPackage>> {
        models::list_packages(&self.readers).await
    }

    /// [`Self::list_packages`] with each archive password read from the vault, for the list a
    /// person sees (RD-104-04).
    pub async fn list_packages_with_passwords(&self) -> Result<Vec<DownloadPackage>> {
        let mut packages = models::list_packages(&self.readers).await?;
        self.reveal_archive_passwords(&mut packages).await;
        Ok(packages)
    }

    /// Returns files in queue order: package priority and position, then the file's position.
    pub async fn list_downloads(&self) -> Result<Vec<DownloadFile>> {
        models::list_downloads(&self.readers).await
    }

    /// One page of [`Self::list_downloads`], cut by SQLite, and how many rows the whole list
    /// holds (RD-1120-17): `offset` rows skipped, then at most `limit` (`None`: the rest).
    pub async fn downloads_page(
        &self,
        offset: u64,
        limit: Option<u64>,
    ) -> Result<(Vec<DownloadFile>, u64)> {
        models::downloads_page(&self.readers, offset, limit).await
    }

    /// Returns one package's files in queue order.
    ///
    /// Callers that only care about one package must use this instead of filtering
    /// [`Self::list_downloads`]: that read loads the whole table, JSON blob columns
    /// included, and the completion check runs it once per finished download.
    pub async fn downloads_for_package(
        &self,
        package_id: rd_core::PackageId,
    ) -> Result<Vec<DownloadFile>> {
        models::downloads_for_package(&self.readers, package_id).await
    }

    /// The `queued` and `retry_wait` rows in queue order, for the dispatcher (audit 1.9.1,
    /// TR-08); whether a retry is due is left to the caller's clock.
    pub async fn startable_downloads(&self) -> Result<Vec<DownloadFile>> {
        models::startable_downloads(&self.readers).await
    }

    /// The committed bytes of every download together — the traffic budget's odometer —
    /// without loading the rows.
    pub async fn committed_bytes_total(&self) -> Result<u64> {
        models::committed_bytes_total(&self.readers).await
    }

    /// One package without its archive password (only `has_password`), or `None`.
    ///
    /// For a caller that wants one package: [`Self::list_packages`] reads the whole table, and
    /// about twenty call sites filtered it for a single id (audit 1.9.1, DB-04). The password
    /// stays behind [`Self::package_password`], like in the list.
    pub async fn get_package(&self, id: rd_core::PackageId) -> Result<Option<DownloadPackage>> {
        models::get_package(&self.readers, id).await
    }

    /// Loads one file.
    pub async fn get_download(&self, id: DownloadId) -> Result<Option<DownloadFile>> {
        models::get_download(&self.readers, id).await
    }
}
