//! Database facade methods for collector, NZB, capture, and destination configuration.

use anyhow::Result;

use crate::{
    Database, NewCategory, NewCategoryRule, NewHotFolder, NewNzbImport, NewStorageRoot,
    automation_store, bandwidth_store, capture_store, collector_store, commands::WriterCommand,
    config_store, mfa_store, notify_store, nzb_store, session_store, writer,
};

impl Database {
    /// Lists LinkGrabber batches newest first.
    pub async fn list_collector_batches(&self) -> Result<Vec<rd_core::CollectorBatch>> {
        collector_store::list_batches(&self.readers).await
    }

    /// Lists LinkGrabber candidates newest first.
    pub async fn list_candidates(&self) -> Result<Vec<rd_core::LinkCandidate>> {
        collector_store::list_candidates(&self.readers).await
    }

    /// Loads a LinkGrabber candidate.
    pub async fn get_candidate(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<rd_core::LinkCandidate>> {
        collector_store::get_candidate(&self.readers, id).await
    }

    /// Applies category (with destination) and/or priority to packages.
    pub async fn update_packages(
        &self,
        ids: Vec<rd_core::PackageId>,
        change: crate::package_store::PackageChange,
    ) -> Result<Vec<rd_core::DownloadPackage>> {
        writer::request(&self.writer, |reply| WriterCommand::UpdatePackages {
            ids,
            change,
            reply,
        })
        .await
    }

    /// Renames a package and points it at a new folder beside its old one (RD-106-13).
    ///
    /// One transaction: the name, the destination, the `previous_destination` the disk move
    /// resumes from, and every absolute path stored for this package. `None` when the id is
    /// unknown.
    pub async fn rename_package_directory(
        &self,
        id: rd_core::PackageId,
        name: String,
        destination: String,
    ) -> Result<Option<rd_core::DownloadPackage>> {
        writer::request(&self.writer, |reply| {
            WriterCommand::RenamePackageDirectory {
                id,
                name,
                destination,
                reply,
            }
        })
        .await
    }

    /// Where the package's files lived before its last category change, while the sweep of
    /// that directory is still outstanding.
    pub async fn package_previous_destination(
        &self,
        id: rd_core::PackageId,
    ) -> Result<Option<String>> {
        crate::package_store::previous_destination(&self.readers, id).await
    }

    /// Marks the outstanding sweep of a package's former directory as done.
    pub async fn clear_package_previous_destination(&self, id: rd_core::PackageId) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::ClearPreviousDestination { id, reply }
        })
        .await
    }

    /// Stores a manual queue order (positions 1..n in the given order).
    pub async fn reorder_packages(&self, ids: Vec<rd_core::PackageId>) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ReorderPackages {
            ids,
            reply,
        })
        .await
    }

    /// Stores a manual file order inside one package (positions 1..n in the given order).
    ///
    /// `ids` has to name exactly the package's files; the API layer rejects anything else, and
    /// the `UPDATE` ignores an id that belongs elsewhere.
    pub async fn reorder_downloads(
        &self,
        package_id: rd_core::PackageId,
        ids: Vec<rd_core::DownloadId>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ReorderDownloads {
            package_id,
            ids,
            reply,
        })
        .await
    }

    /// Removes one LinkGrabber candidate that is not currently being enqueued.
    ///
    /// Its vaulted link fragment goes with it (RD-110-38). Read before the delete, because
    /// the reference is a column of the row being removed: a secret nothing points at is a
    /// leak with a delay.
    pub async fn delete_candidate(&self, id: rd_core::CandidateId) -> Result<()> {
        let orphaned = crate::collector_store::secret_fragment_ref(&self.readers, id)
            .await
            .unwrap_or_default();
        writer::request(&self.writer, |reply| WriterCommand::DeleteCandidate {
            id,
            reply,
        })
        .await?;
        self.forget_secrets(orphaned.into_iter().collect()).await;
        Ok(())
    }

    /// Removes all visible LinkGrabber candidates and returns the affected count.
    ///
    /// The same predicate as the delete itself, so a candidate that is kept -- one being
    /// resolved, one already enqueued -- keeps its secret too.
    pub async fn delete_candidates(&self) -> Result<u64> {
        let orphaned = crate::collector_store::deletable_secret_fragment_refs(&self.readers)
            .await
            .unwrap_or_default();
        let removed = writer::request(&self.writer, |reply| WriterCommand::DeleteCandidates {
            reply,
        })
        .await?;
        self.forget_secrets(orphaned).await;
        Ok(removed)
    }

    /// The `vault://` reference one LinkGrabber candidate holds, if any (RD-110-38).
    ///
    /// The enqueue reads it to move ownership onto the download row. The reference, never
    /// the fragment: what can open it is the vault, and only where the key is needed.
    pub async fn candidate_secret_fragment_ref(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<String>> {
        crate::collector_store::secret_fragment_ref(&self.readers, id).await
    }

    /// The reference a download inherited, and the fragment behind it (RD-110-38).
    ///
    /// The one call that hands back the plaintext, and it is made in exactly one place: the
    /// scheduler, immediately before it asks the stream-transform plugin what lies behind the
    /// address. `None` when the download has no reference, when no vault is installed, or
    /// when the vault refuses -- the resolve then fails on the plugin's own terms rather
    /// than on a half-restored address.
    pub async fn download_secret_fragment(
        &self,
        id: rd_core::DownloadId,
    ) -> Result<Option<String>> {
        use sqlx::Row;

        let row = sqlx::query("SELECT secret_fragment_ref FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.readers)
            .await?;
        let Some(reference) = row.and_then(|row| {
            row.try_get::<Option<String>, _>("secret_fragment_ref")
                .ok()
                .flatten()
        }) else {
            return Ok(None);
        };
        let Some(vault) = self.secret_vault() else {
            return Ok(None);
        };
        let bytes = vault.get_bytes(&reference).await?;
        Ok(Some(String::from_utf8(bytes)?))
    }

    /// The vault reference this download's transform key is reached by, putting the key away
    /// the first time (RD-120-11, ADR 0011).
    ///
    /// The description a stream-transform plugin answers with carries the key itself and no
    /// reference; `rd-http` refuses to build a transform out of one that has no reference,
    /// because the reference is what the fingerprint stands the key on. This is the one place
    /// that closes that gap, and it is deliberately *idempotent*: a second attempt at the same
    /// file gets the same reference back, so the fingerprint is the same and the chunk MACs
    /// the first attempt wrote are still recognisably its own.
    ///
    /// A key that differs from the stored one is a different file behind the same row -- a
    /// re-resolve that came back with another node, or a link whose key was corrected. The old
    /// entry is then removed and a new reference written, which changes the fingerprint and
    /// makes the continuation start over rather than decrypt with one key over bytes written
    /// with another.
    ///
    /// `Ok(None)` means no vault is installed. The caller then has no reference to put in the
    /// description and the transform is refused, which is the right answer: key material that
    /// cannot be put away must not be carried around instead.
    pub async fn adopt_transform_key(
        &self,
        id: rd_core::DownloadId,
        key: &[u8],
    ) -> Result<Option<String>> {
        let Some(vault) = self.secret_vault() else {
            return Ok(None);
        };
        let existing = self.download_transform_key_ref(id).await?;
        if let Some(reference) = &existing
            && vault
                .get_bytes(reference)
                .await
                .is_ok_and(|stored| stored == key)
        {
            return Ok(Some(reference.clone()));
        }
        let reference = vault.put_bytes(key).await?;
        self.set_download_transform_key_ref(id, Some(reference.clone()))
            .await?;
        if let Some(stale) = existing {
            self.forget_secrets(vec![stale]).await;
        }
        Ok(Some(reference))
    }

    /// The reference a download row holds, without opening the vault.
    pub async fn download_transform_key_ref(
        &self,
        id: rd_core::DownloadId,
    ) -> Result<Option<String>> {
        use sqlx::Row;

        let row = sqlx::query("SELECT transform_key_ref FROM downloads WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&self.readers)
            .await?;
        Ok(row.and_then(|row| {
            row.try_get::<Option<String>, _>("transform_key_ref")
                .ok()
                .flatten()
        }))
    }

    /// Writes -- or clears -- that reference.
    pub async fn set_download_transform_key_ref(
        &self,
        id: rd_core::DownloadId,
        reference: Option<String>,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::SetTransformKeyRef {
                id,
                reference,
                reply,
            }
        })
        .await
    }

    /// Persists a parsed NZB and all segment references through the writer actor.
    pub async fn add_nzb_import(&self, import: NewNzbImport) -> Result<rd_core::NzbImport> {
        writer::request(&self.writer, |reply| WriterCommand::AddNzbImport {
            import,
            reply,
        })
        .await
    }

    /// Records an NZB that could not be taken in, so the drop is findable with its reason.
    ///
    /// For the unattended doors above all - a hotfolder drop reaches nobody otherwise, while a
    /// REST caller is handed the same reason in the response (RD-108-20).
    pub async fn record_nzb_import_failure(
        &self,
        failure: crate::FailedNzbImport,
    ) -> Result<rd_core::NzbImport> {
        writer::request(&self.writer, |reply| {
            WriterCommand::RecordNzbImportFailure { failure, reply }
        })
        .await
    }

    /// Changes category and/or priority while an NZB is still in the LinkGrabber.
    pub async fn update_nzb_import(
        &self,
        id: rd_core::NzbImportId,
        change: crate::NzbImportChange,
    ) -> Result<rd_core::NzbImport> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateNzbImport {
            id,
            change,
            reply,
        })
        .await
    }

    /// Moves an imported NZB into the download queue as one package; returns the package.
    ///
    /// With `start_paused` the package is created with every download row paused, so the
    /// scheduler never picks it up until somebody starts it.
    pub async fn enqueue_nzb_import(
        &self,
        id: rd_core::NzbImportId,
        destination: std::path::PathBuf,
        priority: rd_core::DownloadPriority,
        start_paused: bool,
    ) -> Result<rd_core::DownloadPackage> {
        let package_id = writer::request(&self.writer, |reply| WriterCommand::EnqueueNzbImport {
            id,
            destination,
            priority,
            start_paused,
            reply,
        })
        .await?;
        self.list_packages()
            .await?
            .into_iter()
            .find(|package| package.id == package_id)
            .ok_or_else(|| anyhow::anyhow!("queued package not found"))
    }

    /// Lists imported NZBs newest first.
    pub async fn list_nzb_imports(&self) -> Result<Vec<rd_core::NzbImport>> {
        nzb_store::list_imports(&self.readers).await
    }

    /// Lists files and persistent article states for one NZB import.
    pub async fn list_nzb_files(
        &self,
        import_id: rd_core::NzbImportId,
    ) -> Result<Vec<rd_core::NzbFileStatus>> {
        nzb_store::list_files(&self.readers, import_id).await
    }

    /// Drops a completed package's NZB import history (keeps the package itself).
    pub async fn forget_nzb_import_history(&self, package_id: rd_core::PackageId) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::ForgetNzbImportHistory { package_id, reply }
        })
        .await
    }

    /// Removes an inactive NZB import and its cascading segment metadata.
    pub async fn delete_nzb_import(&self, id: rd_core::NzbImportId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteNzbImport {
            id,
            reply,
        })
        .await
    }

    /// Persists one article state and optional verified CRC through the writer actor.
    pub async fn set_nzb_segment_state(
        &self,
        id: rd_core::NzbSegmentId,
        state: rd_core::NzbSegmentState,
        crc32: Option<u32>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetNzbSegmentState {
            id,
            state,
            crc32,
            reply,
        })
        .await
    }

    /// Confirms the atomically promoted output path of one assembled NZB file.
    pub async fn checkpoint_nzb_file_output(
        &self,
        id: rd_core::NzbFileId,
        output_path: String,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::CheckpointNzb {
            checkpoint: crate::postprocess_store::NzbCheckpoint::FileOutput { id, output_path },
            reply,
        })
        .await
    }

    /// Settles a Usenet row once its assembled file is on disk: the final name, the PAR2
    /// marking decided again on that name and on the file's content, and - when the file is
    /// the main index of a set - the postponement of the set's volumes still waiting in the
    /// queue (RD-108-23). Returns how many volumes were postponed.
    pub async fn settle_nzb_recovery(
        &self,
        id: rd_core::DownloadId,
        file_name: String,
        content_is_par2: bool,
    ) -> Result<usize> {
        writer::request(&self.writer, |reply| WriterCommand::SettleNzbRecovery {
            id,
            file_name,
            content_is_par2,
            reply,
        })
        .await
    }

    /// Holds a finished Usenet row's verdict open until its package settles (RD-108-24).
    ///
    /// The file is on disk with zeros where the missing articles belong. Whether that is a
    /// loss depends on the set, and a fully obfuscated set says nothing about its PAR2 files
    /// until they have been assembled - so the row waits in `Verifying`, carrying the reason
    /// it waits, and the answer is given when nothing of the package is on its way any more.
    pub async fn defer_par2_verdict(&self, id: rd_core::DownloadId, missing: usize) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeferPar2Verdict {
            id,
            missing,
            reply,
        })
        .await
    }

    /// Confirms a synced yEnc byte range and its file-level assembly metadata atomically.
    #[allow(clippy::too_many_arguments)]
    pub async fn checkpoint_nzb_assembly_segment(
        &self,
        file_id: rd_core::NzbFileId,
        segment_id: rd_core::NzbSegmentId,
        name: String,
        declared_size: u64,
        part_begin: u64,
        part_end: u64,
        crc32: u32,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::CheckpointNzb {
            checkpoint: crate::postprocess_store::NzbCheckpoint::AssemblySegment {
                file_id,
                segment_id,
                name,
                declared_size,
                part_begin,
                part_end,
                crc32,
            },
            reply,
        })
        .await
    }

    /// Creates or advances one persistent postprocessing checkpoint.
    #[allow(clippy::too_many_arguments)]
    pub async fn checkpoint_postprocess(
        &self,
        owner_id: String,
        kind: rd_core::PostprocessKind,
        source_path: String,
        state: rd_core::PostprocessState,
        output_path: Option<String>,
        message: Option<String>,
    ) -> Result<()> {
        self.checkpoint_postprocess_coded(
            owner_id,
            kind,
            source_path,
            state,
            output_path,
            message,
            None,
            rd_core::MessageParams::new(),
        )
        .await
    }

    /// The same, with a stable code the interface translates and its parameters.
    ///
    /// Separate rather than two more parameters on every existing call: only the outcomes
    /// worth naming carry a code, and the rest should not have to say `None` twice
    /// (RD-107-04).
    #[allow(clippy::too_many_arguments)]
    pub async fn checkpoint_postprocess_coded(
        &self,
        owner_id: String,
        kind: rd_core::PostprocessKind,
        source_path: String,
        state: rd_core::PostprocessState,
        output_path: Option<String>,
        message: Option<String>,
        code: Option<String>,
        params: rd_core::MessageParams,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::CheckpointNzb {
            checkpoint: crate::postprocess_store::NzbCheckpoint::Postprocess {
                owner_id,
                kind,
                source_path,
                state,
                output_path,
                message,
                code,
                params,
                checkpoint: None,
            },
            reply,
        })
        .await
    }

    /// The same, carrying a plugin step's own resume state.
    ///
    /// Separate rather than a seventh parameter on every existing call: only plugin steps
    /// have a checkpoint to keep, and the built-in stages should not have to say `None`.
    #[allow(clippy::too_many_arguments)]
    pub async fn checkpoint_postprocess_with(
        &self,
        owner_id: String,
        kind: rd_core::PostprocessKind,
        source_path: String,
        state: rd_core::PostprocessState,
        output_path: Option<String>,
        message: Option<String>,
        checkpoint: Option<Vec<u8>>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::CheckpointNzb {
            checkpoint: crate::postprocess_store::NzbCheckpoint::Postprocess {
                owner_id,
                kind,
                source_path,
                state,
                output_path,
                message,
                code: None,
                params: rd_core::MessageParams::new(),
                checkpoint,
            },
            reply,
        })
        .await
    }

    /// Pre-registers the ordered post-processing pipeline of a package as `queued` steps.
    pub async fn enqueue_postprocess_steps(
        &self,
        owner_id: String,
        steps: Vec<(rd_core::PostprocessKind, String, i64)>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::CheckpointNzb {
            checkpoint: crate::postprocess_store::NzbCheckpoint::EnqueueSteps { owner_id, steps },
            reply,
        })
        .await
    }

    /// Records live progress of a running step and mirrors stage/percent onto the package.
    pub async fn postprocess_progress(
        &self,
        owner_id: String,
        kind: rd_core::PostprocessKind,
        source_path: String,
        stage: rd_core::PostprocessStage,
        percent: Option<u8>,
        current: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::CheckpointNzb {
            checkpoint: crate::postprocess_store::NzbCheckpoint::Progress {
                owner_id,
                kind,
                source_path,
                stage,
                percent,
                current,
            },
            reply,
        })
        .await
    }

    /// Sets the package lifecycle state (and current post-processing stage).
    pub async fn set_package_state(
        &self,
        id: rd_core::PackageId,
        state: rd_core::PackageState,
        stage: Option<rd_core::PostprocessStage>,
        percent: Option<u8>,
        current: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetPackageState {
            id,
            state,
            stage,
            percent,
            current,
            reply,
        })
        .await
    }

    /// Persists the unpack outcome of the current post-processing run (`None` clears it).
    pub async fn set_package_extraction(
        &self,
        id: rd_core::PackageId,
        result: Option<rd_core::ExtractionResult>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetPackageExtraction {
            id,
            result,
            reply,
        })
        .await
    }

    /// Updates a category's post-processing overrides.
    pub async fn update_category_postprocess(
        &self,
        id: rd_core::CategoryId,
        postprocess: crate::CategoryPostprocess,
    ) -> Result<rd_core::Category> {
        writer::request(&self.writer, |reply| {
            WriterCommand::UpdateCategoryPostprocess {
                id,
                postprocess,
                reply,
            }
        })
        .await
    }

    /// Lists postprocessing checkpoints for one owner (NZB import or package id).
    pub async fn list_postprocess_steps(
        &self,
        owner_id: &str,
    ) -> Result<Vec<rd_core::PostprocessStep>> {
        crate::postprocess_store::list(&self.readers, owner_id).await
    }

    /// Archive password of a package (never serialized; extraction only).
    pub async fn package_password(&self, id: rd_core::PackageId) -> Result<Option<String>> {
        crate::package_store::package_password(&self.readers, id).await
    }

    /// Stores only the SHA-256 digest of a scoped bearer token.
    pub async fn create_capture_token(
        &self,
        id: rd_core::CaptureTokenId,
        label: String,
        token_sha256: String,
        scopes: Vec<String>,
    ) -> Result<rd_core::CaptureToken> {
        writer::request(&self.writer, |reply| WriterCommand::CreateCaptureToken {
            id,
            label,
            token_sha256,
            scopes,
            reply,
        })
        .await
    }

    /// Replaces the scopes of a live token; its bearer value is untouched.
    ///
    /// Deliberately separate from minting rather than an upsert: the two decisions are not the
    /// same one, and a caller that meant to widen a token must not be able to create one by
    /// misspelling an id.
    pub async fn update_capture_token_scopes(
        &self,
        id: rd_core::CaptureTokenId,
        scopes: Vec<String>,
    ) -> Result<rd_core::CaptureToken> {
        writer::request(&self.writer, |reply| {
            WriterCommand::UpdateCaptureTokenScopes { id, scopes, reply }
        })
        .await
    }

    /// Checks a token digest for the given scope without exposing the stored value.
    pub async fn capture_token_valid(&self, token_sha256: &str, scope: &str) -> Result<bool> {
        capture_store::token_valid_with_scope(&self.readers, token_sha256, scope).await
    }

    /// The scopes a live token holds, or `None` if the digest matches no live token.
    pub async fn capture_token_scopes(&self, token_sha256: &str) -> Result<Option<Vec<String>>> {
        capture_store::token_scopes(&self.readers, token_sha256).await
    }

    /// The id, label and scopes of the live token with this digest, for the policy check and
    /// the audit log in one read. `None` when no live token has that digest.
    pub async fn capture_token_identity(
        &self,
        token_sha256: &str,
    ) -> Result<Option<(rd_core::CaptureTokenId, String, Vec<String>)>> {
        capture_store::token_identity(&self.readers, token_sha256).await
    }

    /// Lists revocable connections holding any of the given scopes, without bearer values.
    pub async fn list_capture_tokens(&self, scopes: &[&str]) -> Result<Vec<rd_core::CaptureToken>> {
        capture_store::list_tokens(&self.readers, scopes).await
    }

    /// Opens a session, storing only the digest of its bearer.
    pub async fn create_session(
        &self,
        id: rd_core::SessionId,
        token_sha256: String,
        user_agent: Option<String>,
        client_ip: Option<String>,
        lifetime_hours: i64,
    ) -> Result<rd_core::Session> {
        writer::request(&self.writer, |reply| WriterCommand::CreateSession {
            id,
            token_sha256,
            user_agent,
            client_ip,
            lifetime_hours,
            reply,
        })
        .await
    }

    /// The live session behind a bearer digest under `limits`, read without touching the
    /// writer.
    pub async fn session_for_digest(
        &self,
        token_sha256: &str,
        limits: rd_core::SessionLimits,
    ) -> Result<Option<rd_core::Session>> {
        session_store::session_for_digest(&self.readers, token_sha256, limits).await
    }

    /// Advances a session's last-used time. Reports whether it was still live.
    pub async fn touch_session(&self, token_sha256: String) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::TouchSession {
            token_sha256,
            reply,
        })
        .await
    }

    /// Every live session under `limits`, newest use first.
    pub async fn list_sessions(
        &self,
        limits: rd_core::SessionLimits,
    ) -> Result<Vec<rd_core::Session>> {
        session_store::list_sessions(&self.readers, limits).await
    }

    /// The session limits stored in the service settings (RD-130-09).
    ///
    /// Read here rather than by each caller so the service start, which only holds a
    /// database, and the settings route agree on the field names and on the defaults.
    pub async fn session_limits(&self) -> Result<rd_core::SessionLimits> {
        let idle = self.service_setting_field("session_idle_hours").await?;
        let max = self.service_setting_field("session_max_hours").await?;
        Ok(rd_core::SessionLimits::clamped(idle, max))
    }

    /// Ends one session.
    pub async fn revoke_session(&self, id: rd_core::SessionId) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::RevokeSession {
            id,
            reply,
        })
        .await
    }

    /// Ends every session but the caller's own.
    pub async fn revoke_other_sessions(&self, keep_digest: String) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::RevokeOtherSessions {
            keep_digest,
            reply,
        })
        .await
    }

    /// Ends every session, the caller's own included.
    ///
    /// What a password change needs: a change that leaves the sessions opened with the old
    /// password alive protects nothing (RD-120-22).
    pub async fn revoke_all_sessions(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::RevokeAllSessions {
            reply,
        })
        .await
    }

    /// Deletes session rows that ended under `limits` long ago.
    pub async fn purge_expired_sessions(&self, limits: rd_core::SessionLimits) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::PurgeExpiredSessions {
            limits,
            reply,
        })
        .await
    }

    /// Deletes persisted events past their retention.
    ///
    /// Nothing reads this table today; it is kept as the only record of what happened before
    /// the process started, and the sweep is what keeps that from becoming the largest table
    /// in the file.
    pub async fn purge_old_events(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::PurgeOldEvents {
            reply,
        })
        .await
    }

    /// Notes that a machine token was used, at most once a minute.
    pub async fn touch_capture_token(&self, token_sha256: String) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::TouchCaptureToken {
            token_sha256,
            reply,
        })
        .await
    }

    /// Enrols a second factor. The material itself lives in the secret store.
    pub async fn create_mfa_credential(
        &self,
        id: rd_core::MfaCredentialId,
        kind: rd_core::MfaKind,
        label: String,
        material_ref: String,
    ) -> Result<rd_core::MfaCredential> {
        writer::request(&self.writer, |reply| WriterCommand::CreateMfaCredential {
            id,
            kind,
            label,
            material_ref,
            reply,
        })
        .await
    }

    /// Every enrolled factor, newest first.
    pub async fn list_mfa_credentials(&self) -> Result<Vec<rd_core::MfaCredential>> {
        mfa_store::list_credentials(&self.readers).await
    }

    /// The vault references of every confirmed factor of one kind.
    pub async fn confirmed_mfa_material(
        &self,
        kind: rd_core::MfaKind,
    ) -> Result<Vec<(rd_core::MfaCredentialId, String)>> {
        mfa_store::confirmed_material(&self.readers, kind).await
    }

    /// The vault reference of one factor, confirmed or not.
    pub async fn mfa_material(&self, id: rd_core::MfaCredentialId) -> Result<Option<String>> {
        mfa_store::material_of(&self.readers, id).await
    }

    /// Marks a factor as proven to work.
    pub async fn confirm_mfa_credential(&self, id: rd_core::MfaCredentialId) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::ConfirmMfaCredential {
            id,
            reply,
        })
        .await
    }

    /// Accepts a TOTP code by the time step it answered, and refuses a replay of it.
    ///
    /// Takes the step rather than just noting the use: TOTP accepts a window of one step either
    /// side, so without a record of which step was spent the same six digits keep working for
    /// about ninety seconds. `false` means this step — or a later one — was already accepted,
    /// and the caller must treat the code as wrong.
    pub async fn accept_totp_step(&self, id: rd_core::MfaCredentialId, step: i64) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::AcceptTotpStep {
            id,
            step,
            reply,
        })
        .await
    }

    /// Notes that a factor answered a challenge.
    pub async fn touch_mfa_credential(&self, id: rd_core::MfaCredentialId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::TouchMfaCredential {
            id,
            reply,
        })
        .await
    }

    /// Points a factor at freshly stored material and marks it as just used.
    ///
    /// A passkey's stored state is not static: the signature counter advances and the backup
    /// flags can change, and a counter that is never written back is a counter that can never
    /// detect a cloned authenticator.
    pub async fn repoint_mfa_material(
        &self,
        id: rd_core::MfaCredentialId,
        material_ref: String,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::RepointMfaMaterial {
            id,
            material_ref,
            reply,
        })
        .await
    }

    /// Removes a factor, returning its vault reference so the secret can be deleted too.
    pub async fn delete_mfa_credential(
        &self,
        id: rd_core::MfaCredentialId,
    ) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteMfaCredential {
            id,
            reply,
        })
        .await
    }

    /// Issues a fresh set of recovery codes, invalidating whatever was there.
    pub async fn replace_recovery_codes(&self, digests: Vec<String>) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ReplaceRecoveryCodes {
            digests,
            reply,
        })
        .await
    }

    /// The digests of every unspent recovery code.
    pub async fn unused_recovery_digests(&self) -> Result<Vec<String>> {
        mfa_store::unused_recovery_digests(&self.readers).await
    }

    /// Spends one recovery code. Reports whether it was still unspent.
    pub async fn spend_recovery_code(&self, digest: String) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::SpendRecoveryCode {
            digest,
            reply,
        })
        .await
    }

    /// Removes every factor of one kind plus every recovery code, returning the vault
    /// references so the caller can delete the material behind them.
    pub async fn clear_mfa(&self, kind: rd_core::MfaKind) -> Result<Vec<String>> {
        writer::request(&self.writer, |reply| WriterCommand::ClearMfa {
            kind,
            reply,
        })
        .await
    }

    /// Revokes one capture connection without deleting its audit metadata.
    pub async fn revoke_capture_token(&self, id: rd_core::CaptureTokenId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::RevokeCaptureToken {
            id,
            reply,
        })
        .await
    }

    /// Adds an allowlisted download destination.
    /// Creates a root under the id the caller already materialised the directory with.
    pub async fn create_storage_root(
        &self,
        id: rd_core::StorageRootId,
        input: NewStorageRoot,
    ) -> Result<rd_core::StorageRootConfig> {
        writer::request(&self.writer, |reply| WriterCommand::CreateStorageRoot {
            id,
            input,
            reply,
        })
        .await
    }

    /// Lists all allowlisted download destinations.
    pub async fn list_storage_roots(&self) -> Result<Vec<rd_core::StorageRootConfig>> {
        config_store::list_storage_roots(&self.readers).await
    }

    /// The storage root new downloads land on when no category picks one.
    pub async fn default_storage_root(&self) -> Result<Option<rd_core::StorageRootConfig>> {
        config_store::default_storage_root(&self.readers).await
    }

    /// Adds a category mapped to one storage root and relative destination.
    pub async fn create_category(&self, input: NewCategory) -> Result<rd_core::Category> {
        writer::request(&self.writer, |reply| WriterCommand::CreateCategory {
            input,
            reply,
        })
        .await
    }

    /// Lists categories in display order.
    pub async fn list_categories(&self) -> Result<Vec<rd_core::Category>> {
        config_store::list_categories(&self.readers).await
    }

    /// Adds one prioritized first-match category rule.
    pub async fn create_category_rule(
        &self,
        input: NewCategoryRule,
    ) -> Result<rd_core::CategoryRule> {
        writer::request(&self.writer, |reply| WriterCommand::CreateCategoryRule {
            input,
            reply,
        })
        .await
    }

    /// Lists category rules by priority.
    pub async fn list_category_rules(&self) -> Result<Vec<rd_core::CategoryRule>> {
        config_store::list_category_rules(&self.readers).await
    }

    /// Persists one daemon or capture-agent hotfolder.
    pub async fn create_hotfolder(&self, input: NewHotFolder) -> Result<rd_core::HotFolderConfig> {
        writer::request(&self.writer, |reply| WriterCommand::CreateHotFolder {
            input,
            reply,
        })
        .await
    }

    /// Lists configured daemon and capture-agent hotfolders.
    pub async fn list_hotfolders(&self) -> Result<Vec<rd_core::HotFolderConfig>> {
        config_store::list_hotfolders(&self.readers).await
    }

    /// Lists the livestream channels watched by the recording monitor.
    pub async fn list_stream_channels(&self) -> Result<Vec<rd_core::StreamChannel>> {
        crate::stream_store::list(&self.readers).await
    }

    /// Lists every auth profile, including disabled and expired ones.
    pub async fn list_auth_profiles(&self) -> Result<Vec<rd_core::AuthProfile>> {
        crate::auth_profile_store::list(&self.readers).await
    }

    pub async fn auth_profile(
        &self,
        id: rd_core::AuthProfileId,
    ) -> Result<Option<rd_core::AuthProfile>> {
        crate::auth_profile_store::get(&self.readers, id).await
    }

    /// Most specific enabled, unexpired profile whose scope covers `url`.
    pub async fn match_auth_profile(&self, url: &url::Url) -> Result<Option<rd_core::AuthProfile>> {
        crate::auth_profile_store::match_for_url(&self.readers, url).await
    }

    /// Points one download at a profile, at none, or back at scope matching.
    pub async fn set_download_auth_profile(
        &self,
        id: rd_core::DownloadId,
        selection: rd_core::AuthProfileSelection,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::SetDownloadAuthProfile {
                id,
                selection,
                reply,
            }
        })
        .await
    }

    pub async fn create_auth_profile(
        &self,
        input: crate::auth_profile_store::NewAuthProfile,
    ) -> Result<rd_core::AuthProfile> {
        writer::request(&self.writer, |reply| WriterCommand::CreateAuthProfile {
            input,
            reply,
        })
        .await
    }

    /// Updates a profile and returns it together with the secret references that fell out
    /// of use, so the caller can remove them from the secret store.
    pub async fn update_auth_profile(
        &self,
        id: rd_core::AuthProfileId,
        input: crate::auth_profile_store::UpdateAuthProfile,
    ) -> Result<(rd_core::AuthProfile, Vec<String>)> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateAuthProfile {
            id,
            input,
            reply,
        })
        .await
    }

    /// Enables or disables a profile; this is how a captured browser session is approved.
    pub async fn set_auth_profile_enabled(
        &self,
        id: rd_core::AuthProfileId,
        enabled: bool,
    ) -> Result<rd_core::AuthProfile> {
        writer::request(&self.writer, |reply| WriterCommand::SetAuthProfileEnabled {
            id,
            enabled,
            reply,
        })
        .await
    }

    /// Deletes a profile and returns the secret references it orphaned.
    pub async fn delete_auth_profile(&self, id: rd_core::AuthProfileId) -> Result<Vec<String>> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteAuthProfile {
            id,
            reply,
        })
        .await
    }

    /// Lists every stored FTP/SFTP login, including disabled ones.
    pub async fn list_remote_credentials(&self) -> Result<Vec<rd_core::RemoteCredential>> {
        crate::remote_store::list(&self.readers).await
    }

    pub async fn remote_credential(
        &self,
        id: rd_core::RemoteCredentialId,
    ) -> Result<Option<rd_core::RemoteCredential>> {
        crate::remote_store::get(&self.readers, id).await
    }

    /// Most specific enabled login that can reach `target`.
    pub async fn match_remote_credential(
        &self,
        target: &rd_core::RemoteTarget,
    ) -> Result<Option<rd_core::RemoteCredential>> {
        crate::remote_store::match_for_target(&self.readers, target).await
    }

    pub async fn create_remote_credential(
        &self,
        input: crate::remote_store::NewRemoteCredential,
    ) -> Result<rd_core::RemoteCredential> {
        writer::request(&self.writer, |reply| {
            WriterCommand::CreateRemoteCredential {
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    /// Updates a login and returns it together with the secret references that fell out of
    /// use, so the caller can remove them from the secret store.
    pub async fn update_remote_credential(
        &self,
        id: rd_core::RemoteCredentialId,
        input: crate::remote_store::UpdateRemoteCredential,
    ) -> Result<(rd_core::RemoteCredential, Vec<String>)> {
        writer::request(&self.writer, |reply| {
            WriterCommand::UpdateRemoteCredential {
                id,
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    /// Deletes a login and returns the secret references it orphaned.
    pub async fn delete_remote_credential(
        &self,
        id: rd_core::RemoteCredentialId,
    ) -> Result<Vec<String>> {
        writer::request(&self.writer, |reply| {
            WriterCommand::DeleteRemoteCredential { id, reply }
        })
        .await
    }

    /// What the trust store says about a server key that was just offered.
    pub async fn ssh_host_key_verdict(
        &self,
        host: &str,
        port: u16,
        algorithm: &str,
        fingerprint: &str,
    ) -> Result<crate::remote_store::HostKeyVerdict> {
        crate::remote_store::host_key_verdict(&self.readers, host, port, algorithm, fingerprint)
            .await
    }

    pub async fn list_ssh_host_keys(&self) -> Result<Vec<rd_core::SshHostKey>> {
        crate::remote_store::list_host_keys(&self.readers).await
    }

    /// Records a server key as trusted. Overwrites an existing entry, so the caller must
    /// have made a *changed* key an explicit decision before getting here.
    pub async fn trust_ssh_host_key(&self, key: rd_core::SshHostKey) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::TrustSshHostKey {
            key: Box::new(key),
            reply,
        })
        .await
    }

    pub async fn forget_ssh_host_key(
        &self,
        host: String,
        port: u16,
        algorithm: String,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::ForgetSshHostKey {
            host,
            port,
            algorithm,
            reply,
        })
        .await
    }

    /// The reviewed remote directory listing of one link candidate.
    pub async fn candidate_listing(
        &self,
        id: rd_core::CandidateId,
    ) -> Result<Option<rd_core::RemoteCandidateState>> {
        crate::remote_store::candidate_listing(&self.readers, id).await
    }

    /// Stores a probed listing on a candidate, preserving an existing selection for the
    /// same directory root.
    pub async fn set_candidate_listing(
        &self,
        id: rd_core::CandidateId,
        listing: rd_core::RemoteListing,
        credential_id: Option<rd_core::RemoteCredentialId>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::SetCandidateListing {
            id,
            listing: Box::new(listing),
            credential_id,
            reply,
        })
        .await
    }

    /// Replaces the file selection of one candidate and returns the resolved listing.
    pub async fn set_candidate_listing_plan(
        &self,
        id: rd_core::CandidateId,
        plan: rd_core::RemoteListingPlan,
    ) -> Result<rd_core::ResolvedRemoteListing> {
        writer::request(&self.writer, |reply| {
            WriterCommand::SetCandidateListingPlan { id, plan, reply }
        })
        .await
    }

    pub async fn create_stream_channel(
        &self,
        input: crate::stream_store::NewStreamChannel,
    ) -> Result<rd_core::StreamChannel> {
        writer::request(&self.writer, |reply| WriterCommand::CreateStreamChannel {
            input,
            reply,
        })
        .await
    }

    pub async fn update_stream_channel(
        &self,
        id: rd_core::StreamChannelId,
        input: crate::stream_store::NewStreamChannel,
    ) -> Result<rd_core::StreamChannel> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateStreamChannel {
            id,
            input,
            reply,
        })
        .await
    }

    pub async fn delete_stream_channel(&self, id: rd_core::StreamChannelId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteStreamChannel {
            id,
            reply,
        })
        .await
    }

    /// Records a monitor probe outcome (live timestamp and/or latest error).
    pub async fn touch_stream_channel(
        &self,
        id: rd_core::StreamChannelId,
        live_at: Option<chrono::DateTime<chrono::Utc>>,
        error: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::TouchStreamChannel {
            id,
            live_at,
            error,
            reply,
        })
        .await
    }

    /// Replaces a storage root's name, path and default flag.
    pub async fn update_storage_root(
        &self,
        id: rd_core::StorageRootId,
        input: NewStorageRoot,
    ) -> Result<rd_core::StorageRootConfig> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateStorageRoot {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes a storage root; fails while categories still point at it.
    pub async fn delete_storage_root(&self, id: rd_core::StorageRootId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteStorageRoot {
            id,
            reply,
        })
        .await
    }

    /// Replaces every editable field of a category.
    pub async fn update_category(
        &self,
        id: rd_core::CategoryId,
        input: NewCategory,
    ) -> Result<rd_core::Category> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateCategory {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes a category and its rules; fails while unfinished packages use it.
    pub async fn delete_category(&self, id: rd_core::CategoryId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteCategory {
            id,
            reply,
        })
        .await
    }

    /// Replaces every editable field of a category rule.
    pub async fn update_category_rule(
        &self,
        id: rd_core::CategoryRuleId,
        input: NewCategoryRule,
    ) -> Result<rd_core::CategoryRule> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateCategoryRule {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes one category rule.
    pub async fn delete_category_rule(&self, id: rd_core::CategoryRuleId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteCategoryRule {
            id,
            reply,
        })
        .await
    }

    /// Replaces every editable field of a hotfolder.
    pub async fn update_hotfolder(
        &self,
        id: rd_core::HotFolderId,
        input: NewHotFolder,
    ) -> Result<rd_core::HotFolderConfig> {
        writer::request(&self.writer, |reply| WriterCommand::UpdateHotFolder {
            id,
            input,
            reply,
        })
        .await
    }

    /// Removes one hotfolder; the caller stops its watcher.
    pub async fn delete_hotfolder(&self, id: rd_core::HotFolderId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteHotFolder {
            id,
            reply,
        })
        .await
    }
}

/// Bandwidth profiles, their weekly schedule and the traffic counters (RD-050-12).
impl Database {
    pub async fn list_bandwidth_profiles(&self) -> Result<Vec<rd_limits::BandwidthProfile>> {
        bandwidth_store::list_profiles(&self.readers).await
    }

    pub async fn list_bandwidth_windows(&self) -> Result<Vec<rd_limits::ScheduleWindow>> {
        bandwidth_store::list_windows(&self.readers).await
    }

    pub async fn create_bandwidth_profile(
        &self,
        input: bandwidth_store::NewBandwidthProfile,
    ) -> Result<rd_limits::BandwidthProfile> {
        writer::request(&self.writer, |reply| {
            WriterCommand::CreateBandwidthProfile { input, reply }
        })
        .await
    }

    pub async fn update_bandwidth_profile(
        &self,
        id: rd_core::BandwidthProfileId,
        input: bandwidth_store::NewBandwidthProfile,
    ) -> Result<rd_limits::BandwidthProfile> {
        writer::request(&self.writer, |reply| {
            WriterCommand::UpdateBandwidthProfile { id, input, reply }
        })
        .await
    }

    pub async fn delete_bandwidth_profile(&self, id: rd_core::BandwidthProfileId) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::DeleteBandwidthProfile { id, reply }
        })
        .await
    }

    /// Replaces the weekly schedule as one document.
    pub async fn replace_bandwidth_windows(
        &self,
        windows: Vec<bandwidth_store::NewScheduleWindow>,
    ) -> Result<Vec<rd_limits::ScheduleWindow>> {
        writer::request(&self.writer, |reply| {
            WriterCommand::ReplaceBandwidthWindows { windows, reply }
        })
        .await
    }

    pub async fn bandwidth_budgets(&self) -> Result<rd_limits::BudgetStates> {
        bandwidth_store::budget_states(&self.readers).await
    }

    pub async fn store_bandwidth_budget(
        &self,
        profile_id: rd_core::BandwidthProfileId,
        state: rd_limits::BudgetState,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::StoreBandwidthBudget {
            profile_id,
            state,
            reply,
        })
        .await
    }
}

/// Automations, their versions and the run history (RD-090-04).
impl Database {
    pub async fn list_automations(&self) -> Result<Vec<rd_automation::Automation>> {
        automation_store::list(&self.readers).await
    }

    /// The definition in force for every enabled automation.
    pub async fn active_automation_versions(
        &self,
    ) -> Result<Vec<rd_automation::AutomationVersion>> {
        automation_store::active_versions(&self.readers).await
    }

    pub async fn automation_version(
        &self,
        id: rd_core::AutomationVersionId,
    ) -> Result<Option<rd_automation::AutomationVersion>> {
        automation_store::version(&self.readers, id).await
    }

    pub async fn automation_versions(
        &self,
        automation_id: rd_core::AutomationId,
    ) -> Result<Vec<rd_automation::AutomationVersion>> {
        automation_store::versions(&self.readers, automation_id).await
    }

    pub async fn automation_runs(
        &self,
        automation_id: Option<rd_core::AutomationId>,
        limit: u32,
    ) -> Result<Vec<rd_automation::Run>> {
        automation_store::runs(&self.readers, automation_id, limit).await
    }

    /// Runs waiting to start, and retries that have come due.
    pub async fn due_automation_runs(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_automation::Run>> {
        automation_store::due_runs(&self.readers, now).await
    }

    pub async fn upsert_automation(
        &self,
        id: Option<rd_core::AutomationId>,
        input: automation_store::NewAutomation,
    ) -> Result<rd_automation::Automation> {
        writer::request(&self.writer, |reply| WriterCommand::UpsertAutomation {
            id,
            input,
            reply,
        })
        .await
    }

    pub async fn set_automation_enabled(
        &self,
        id: rd_core::AutomationId,
        enabled: bool,
    ) -> Result<rd_automation::Automation> {
        writer::request(&self.writer, |reply| WriterCommand::SetAutomationEnabled {
            id,
            enabled,
            reply,
        })
        .await
    }

    pub async fn delete_automation(&self, id: rd_core::AutomationId) -> Result<()> {
        writer::request(&self.writer, |reply| WriterCommand::DeleteAutomation {
            id,
            reply,
        })
        .await
    }

    /// Queues a run; `false` means this event already produced one for this version.
    pub async fn queue_automation_run(&self, input: automation_store::NewRun) -> Result<bool> {
        writer::request(&self.writer, |reply| WriterCommand::QueueAutomationRun {
            input,
            reply,
        })
        .await
    }

    pub async fn record_automation_attempt(
        &self,
        id: rd_core::AutomationRunId,
        state: rd_automation::RunState,
        action_index: u32,
        attempt: u32,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        message: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::RecordAutomationAttempt {
                id,
                state,
                action_index,
                attempt,
                next_attempt_at,
                message,
                reply,
            }
        })
        .await
    }

    /// Re-queues runs that were mid-flight when the service stopped.
    pub async fn recover_automation_runs(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| WriterCommand::RecoverAutomationRuns {
            reply,
        })
        .await
    }
}

/// Notification targets, rules and the delivery history (RD-050-14).
impl Database {
    pub async fn list_notification_targets(&self) -> Result<Vec<rd_notify::NotificationTarget>> {
        notify_store::list_targets(&self.readers).await
    }

    pub async fn list_notification_rules(&self) -> Result<Vec<rd_notify::NotificationRule>> {
        notify_store::list_rules(&self.readers).await
    }

    pub async fn list_notification_deliveries(
        &self,
        limit: u32,
    ) -> Result<Vec<rd_notify::Delivery>> {
        notify_store::list_deliveries(&self.readers, limit).await
    }

    /// How many deliveries a clear of the history would remove right now: every one except
    /// those still queued or retrying (RD-130-08).
    pub async fn count_clearable_notification_deliveries(&self) -> Result<u64> {
        notify_store::count_clearable_deliveries(&self.readers).await
    }

    /// Deliveries whose next attempt is due.
    pub async fn due_notification_deliveries(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_notify::Delivery>> {
        notify_store::due_deliveries(&self.readers, now).await
    }

    pub async fn upsert_notification_target(
        &self,
        id: Option<rd_core::NotificationTargetId>,
        input: notify_store::NewNotificationTarget,
    ) -> Result<rd_notify::NotificationTarget> {
        writer::request(&self.writer, |reply| {
            WriterCommand::UpsertNotificationTarget { id, input, reply }
        })
        .await
    }

    /// Removes a target and returns its vault reference, so the caller can drop the secret.
    pub async fn delete_notification_target(
        &self,
        id: rd_core::NotificationTargetId,
    ) -> Result<Option<String>> {
        writer::request(&self.writer, |reply| {
            WriterCommand::DeleteNotificationTarget { id, reply }
        })
        .await
    }

    pub async fn upsert_notification_rule(
        &self,
        id: Option<rd_core::NotificationRuleId>,
        input: notify_store::NewNotificationRule,
    ) -> Result<rd_notify::NotificationRule> {
        writer::request(&self.writer, |reply| {
            WriterCommand::UpsertNotificationRule { id, input, reply }
        })
        .await
    }

    pub async fn delete_notification_rule(&self, id: rd_core::NotificationRuleId) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::DeleteNotificationRule { id, reply }
        })
        .await
    }

    /// Queues a delivery; `false` means the idempotency key was already taken.
    pub async fn queue_notification_delivery(
        &self,
        input: notify_store::NewDelivery,
    ) -> Result<bool> {
        writer::request(&self.writer, |reply| {
            WriterCommand::QueueNotificationDelivery { input, reply }
        })
        .await
    }

    pub async fn record_notification_attempt(
        &self,
        id: rd_core::NotificationDeliveryId,
        state: rd_notify::DeliveryState,
        attempt: u32,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        response_status: Option<u16>,
        response_excerpt: Option<String>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| {
            WriterCommand::RecordNotificationAttempt {
                id,
                state,
                attempt,
                next_attempt_at,
                response_status,
                response_excerpt,
                reply,
            }
        })
        .await
    }

    /// Empties the delivery history and reports how many rows went. Deliveries still queued
    /// or retrying stay: the worker owes them an attempt (RD-130-08).
    pub async fn clear_notification_deliveries(&self) -> Result<u64> {
        writer::request(&self.writer, |reply| {
            WriterCommand::ClearNotificationDeliveries { reply }
        })
        .await
    }
    // ---- Subscriptions (RD-080-07) ----

    pub async fn list_subscriptions(&self) -> Result<Vec<rd_core::Subscription>> {
        crate::subscription_store::list(&self.readers).await
    }

    pub async fn subscription(
        &self,
        id: rd_core::SubscriptionId,
    ) -> Result<Option<rd_core::Subscription>> {
        crate::subscription_store::get(&self.readers, id).await
    }

    /// Subscriptions whose next poll is due at `now`, oldest first.
    pub async fn due_subscriptions(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_core::Subscription>> {
        crate::subscription_store::due(&self.readers, now).await
    }

    pub async fn subscription_items(
        &self,
        id: rd_core::SubscriptionId,
        limit: i64,
    ) -> Result<Vec<rd_core::SubscriptionItem>> {
        crate::subscription_store::items(&self.readers, id, limit).await
    }

    pub async fn subscription_item_page(
        &self,
        id: rd_core::SubscriptionId,
        state: Option<rd_core::SubscriptionItemState>,
        limit: i64,
        offset: i64,
    ) -> Result<rd_core::SubscriptionItemPage> {
        crate::subscription_store::item_page(&self.readers, id, state, limit, offset).await
    }

    pub async fn subscription_review_summary(&self) -> Result<rd_core::SubscriptionReviewSummary> {
        crate::subscription_store::review_summary(&self.readers).await
    }

    pub async fn pending_subscription_item_ids(
        &self,
        id: rd_core::SubscriptionId,
    ) -> Result<Vec<rd_core::SubscriptionItemId>> {
        crate::subscription_store::pending_item_ids(&self.readers, id).await
    }

    /// One subscription item by id.
    pub async fn subscription_item(
        &self,
        id: rd_core::SubscriptionItemId,
    ) -> Result<Option<rd_core::SubscriptionItem>> {
        crate::subscription_store::item(&self.readers, id).await
    }

    pub async fn subscription_runs(
        &self,
        id: rd_core::SubscriptionId,
        limit: i64,
    ) -> Result<Vec<rd_core::SubscriptionRun>> {
        crate::subscription_store::runs(&self.readers, id, limit).await
    }

    pub async fn create_subscription(
        &self,
        input: crate::subscription_store::NewSubscription,
    ) -> Result<rd_core::Subscription> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::CreateSubscription {
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    /// Applies an edit and returns the subscription plus any secret reference it replaced,
    /// which the caller removes from the vault.
    pub async fn update_subscription(
        &self,
        id: rd_core::SubscriptionId,
        input: crate::subscription_store::NewSubscription,
    ) -> Result<(rd_core::Subscription, Option<String>)> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::UpdateSubscription {
                id,
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    pub async fn set_subscription_enabled(
        &self,
        id: rd_core::SubscriptionId,
        enabled: bool,
    ) -> Result<rd_core::Subscription> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::SetSubscriptionEnabled { id, enabled, reply }
        })
        .await
    }

    /// Deletes a subscription and its archive, returning its secret reference if it had one.
    pub async fn delete_subscription(&self, id: rd_core::SubscriptionId) -> Result<Option<String>> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::DeleteSubscription { id, reply }
        })
        .await
    }

    /// Archives what a poll found and returns only the rows that were new.
    pub async fn record_subscription_items(
        &self,
        subscription_id: rd_core::SubscriptionId,
        items: Vec<crate::subscription_store::NewSubscriptionItem>,
    ) -> Result<Vec<rd_core::SubscriptionItem>> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::RecordSubscriptionItems {
                subscription_id,
                items,
                reply,
            }
        })
        .await
    }

    pub async fn set_subscription_item_state(
        &self,
        id: rd_core::SubscriptionItemId,
        state: rd_core::SubscriptionItemState,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::SetSubscriptionItemState { id, state, reply }
        })
        .await
    }

    pub async fn set_pending_subscription_items_state(
        &self,
        ids: Vec<rd_core::SubscriptionItemId>,
        state: rd_core::SubscriptionItemState,
    ) -> Result<u64> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::SetPendingSubscriptionItemsState { ids, state, reply }
        })
        .await
    }

    pub async fn clear_subscription_history(
        &self,
        id: rd_core::SubscriptionId,
    ) -> Result<rd_core::SubscriptionHistoryClearResponse> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::ClearSubscriptionHistory { id, reply }
        })
        .await
    }

    /// Gives a scheduled subscription that was never timed its first due time (RD-130-19).
    ///
    /// Returns whether the row was armed; one that already has a next run is left alone.
    pub async fn arm_subscription(
        &self,
        id: rd_core::SubscriptionId,
        next_run_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::ArmSubscription {
                id,
                next_run_at,
                reply,
            }
        })
        .await
    }

    pub async fn finish_subscription_run(
        &self,
        subscription_id: rd_core::SubscriptionId,
        started_at: chrono::DateTime<chrono::Utc>,
        result: crate::subscription_store::PollResult,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::FinishSubscriptionRun {
                subscription_id,
                started_at,
                result: Box::new(result),
                reply,
            }
        })
        .await
    }
    /// Stores a recording's segment history and sidecars (RD-080-09).
    pub async fn set_download_recording_state(
        &self,
        id: rd_core::DownloadId,
        state: rd_core::RecordingState,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::SetDownloadRecordingState {
                id,
                state: Box::new(state),
                reply,
            }
        })
        .await
    }

    // ---- Livestream schedules (RD-080-08) ----

    pub async fn list_stream_schedules(&self) -> Result<Vec<rd_core::StreamSchedule>> {
        crate::stream_schedule_store::list(&self.readers).await
    }

    pub async fn enabled_stream_schedules(&self) -> Result<Vec<rd_core::StreamSchedule>> {
        crate::stream_schedule_store::enabled(&self.readers).await
    }

    pub async fn stream_scheduled_runs(
        &self,
        schedule_id: Option<rd_core::StreamScheduleId>,
        limit: i64,
    ) -> Result<Vec<rd_core::StreamScheduledRun>> {
        crate::stream_schedule_store::runs(&self.readers, schedule_id, limit).await
    }

    /// Runs that still need the monitor's attention.
    pub async fn open_stream_runs(&self) -> Result<Vec<rd_core::StreamScheduledRun>> {
        crate::stream_schedule_store::open_runs(&self.readers).await
    }

    pub async fn create_stream_schedule(
        &self,
        input: crate::stream_schedule_store::NewStreamSchedule,
    ) -> Result<rd_core::StreamSchedule> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::CreateStreamSchedule {
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    pub async fn update_stream_schedule(
        &self,
        id: rd_core::StreamScheduleId,
        input: crate::stream_schedule_store::NewStreamSchedule,
    ) -> Result<rd_core::StreamSchedule> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::UpdateStreamSchedule {
                id,
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    pub async fn delete_stream_schedule(&self, id: rd_core::StreamScheduleId) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::DeleteStreamSchedule { id, reply }
        })
        .await
    }

    /// Plans occurrences, skipping any already planned. Returns how many were new.
    pub async fn plan_stream_runs(
        &self,
        schedule_id: rd_core::StreamScheduleId,
        channel_id: rd_core::StreamChannelId,
        occurrences: Vec<crate::stream_schedule_store::PlannedOccurrence>,
    ) -> Result<u32> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::PlanStreamRuns {
                schedule_id,
                channel_id,
                occurrences,
                reply,
            }
        })
        .await
    }

    pub async fn set_stream_run_state(
        &self,
        id: rd_core::StreamScheduledRunId,
        state: rd_core::ScheduledRunState,
        download_id: Option<rd_core::DownloadId>,
        replay_used: Option<bool>,
        error: Option<String>,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::SetStreamRunState {
                id,
                state,
                download_id,
                replay_used,
                error,
                reply,
            }
        })
        .await
    }

    /// Marks open runs whose window closed before `cutoff` as missed.
    pub async fn expire_stream_runs(&self, cutoff: chrono::DateTime<chrono::Utc>) -> Result<u32> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::WriterCommand::ExpireStreamRuns { cutoff, reply }
        })
        .await
    }
}
