//! Database facade for NZB imports, their files, segments, assembly and repair checkpoints.

use anyhow::Result;

use crate::{
    Database, NewNzbImport, archive_password::PasswordTable, commands::NzbCommand, nzb_store,
    writer,
};

impl Database {
    /// Persists a parsed NZB and all segment references through the writer actor.
    ///
    /// Its archive password goes into the vault once the row exists (RD-190-04). A file that
    /// is already here keeps the password it arrived with the first time.
    pub async fn add_nzb_import(&self, mut import: NewNzbImport) -> Result<rd_core::NzbImport> {
        import.password = import.password.filter(|value| !value.is_empty());
        let password = import.password.clone();
        let mut added = writer::request(&self.writer, |reply| NzbCommand::AddNzbImport {
            import,
            reply,
        })
        .await?;
        if added.duplicate {
            self.reveal_archive_passwords(std::slice::from_mut(&mut added))
                .await;
        } else if password.is_some()
            && let Err(error) = self
                .store_archive_passwords(
                    PasswordTable::NzbImports,
                    vec![(added.id.to_string(), password)],
                )
                .await
        {
            // The import is in; a vault that refuses costs it the password, not the intake.
            tracing::warn!(%error, import_id = %added.id, "the archive password of an NZB import could not be put in the vault");
            added.has_password = false;
            added.password = None;
        }
        Ok(added)
    }

    /// Records an NZB that could not be taken in, so the drop is findable with its reason.
    ///
    /// For the unattended doors above all - a hotfolder drop reaches nobody otherwise, while a
    /// REST caller is handed the same reason in the response (RD-108-20).
    pub async fn record_nzb_import_failure(
        &self,
        failure: crate::FailedNzbImport,
    ) -> Result<rd_core::NzbImport> {
        writer::request(&self.writer, |reply| NzbCommand::RecordNzbImportFailure {
            failure,
            reply,
        })
        .await
    }

    /// Changes category and/or priority while an NZB is still in the LinkGrabber.
    pub async fn update_nzb_import(
        &self,
        id: rd_core::NzbImportId,
        change: crate::NzbImportChange,
    ) -> Result<rd_core::NzbImport> {
        let mut updated = writer::request(&self.writer, |reply| NzbCommand::UpdateNzbImport {
            id,
            change,
            reply,
        })
        .await?;
        self.reveal_archive_passwords(std::slice::from_mut(&mut updated))
            .await;
        Ok(updated)
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
        // The package gets a vault entry of its own with the same password, so forgetting the
        // import's history can never take the package's password with it (RD-190-04).
        let password = self
            .archive_password(PasswordTable::NzbImports, id.to_string())
            .await?;
        let package_id = writer::request(&self.writer, |reply| NzbCommand::EnqueueNzbImport {
            id,
            destination,
            priority,
            start_paused,
            reply,
        })
        .await?;
        if password.is_some()
            && let Err(error) = self
                .store_archive_passwords(
                    PasswordTable::Packages,
                    vec![(package_id.to_string(), password)],
                )
                .await
        {
            // The package is queued; a vault that refuses costs it the password, not the queue.
            tracing::warn!(%error, %package_id, "the archive password of a queued NZB could not be put in the vault");
        }
        let mut package = self
            .get_package(package_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("queued package not found"))?;
        self.reveal_archive_passwords(std::slice::from_mut(&mut package))
            .await;
        Ok(package)
    }

    /// Lists imported NZBs newest first, with their archive passwords (RD-104-04).
    pub async fn list_nzb_imports(&self) -> Result<Vec<rd_core::NzbImport>> {
        let mut imports = nzb_store::list_imports(&self.readers).await?;
        self.reveal_archive_passwords(&mut imports).await;
        Ok(imports)
    }

    /// One page of [`Self::list_nzb_imports`], cut by SQLite, and how many imports there are
    /// (RD-191-05); only the page's passwords are read from the vault.
    pub async fn nzb_imports_page(
        &self,
        offset: u64,
        limit: Option<u64>,
    ) -> Result<(Vec<rd_core::NzbImport>, u64)> {
        let (mut imports, total) = nzb_store::imports_page(&self.readers, offset, limit).await?;
        self.reveal_archive_passwords(&mut imports).await;
        Ok((imports, total))
    }

    /// One NZB import with its archive password, or `None`.
    pub async fn get_nzb_import(
        &self,
        id: rd_core::NzbImportId,
    ) -> Result<Option<rd_core::NzbImport>> {
        let Some(mut import) = nzb_store::get_import(&self.readers, id).await? else {
            return Ok(None);
        };
        self.reveal_archive_passwords(std::slice::from_mut(&mut import))
            .await;
        Ok(Some(import))
    }

    /// Records that an NZB import was handed to a remote job (RD-191-13): from the LinkGrabber
    /// (`expected` = `Imported`) or behind a queued package (`Enqueued`). Refused as
    /// `WrongState` when the import has moved on from `expected`.
    pub async fn mark_nzb_import_remote_job(
        &self,
        id: rd_core::NzbImportId,
        remote_job_id: rd_core::RemoteJobId,
        expected: rd_core::NzbImportState,
    ) -> Result<rd_core::NzbImport> {
        let mut updated =
            writer::request(&self.writer, |reply| NzbCommand::MarkNzbImportRemoteJob {
                id,
                remote_job_id,
                expected,
                reply,
            })
            .await?;
        self.reveal_archive_passwords(std::slice::from_mut(&mut updated))
            .await;
        Ok(updated)
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
        writer::request(&self.writer, |reply| NzbCommand::ForgetNzbImportHistory {
            package_id,
            reply,
        })
        .await?;
        self.sweep_archive_passwords().await;
        Ok(())
    }

    /// Removes an inactive NZB import and its cascading segment metadata, and its archive
    /// password from the vault.
    pub async fn delete_nzb_import(&self, id: rd_core::NzbImportId) -> Result<()> {
        writer::request(&self.writer, |reply| NzbCommand::DeleteNzbImport {
            id,
            reply,
        })
        .await?;
        self.sweep_archive_passwords().await;
        Ok(())
    }

    /// Persists one article state and optional verified CRC through the writer actor.
    pub async fn set_nzb_segment_state(
        &self,
        id: rd_core::NzbSegmentId,
        state: rd_core::NzbSegmentState,
        crc32: Option<u32>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| NzbCommand::SetNzbSegmentState {
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
        writer::request(&self.writer, |reply| NzbCommand::CheckpointNzb {
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
        writer::request(&self.writer, |reply| NzbCommand::SettleNzbRecovery {
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
        writer::request(&self.writer, |reply| NzbCommand::DeferPar2Verdict {
            id,
            missing,
            reply,
        })
        .await
    }

    /// Confirms several written articles of one file in one transaction (RD-130-22).
    ///
    /// All or nothing: a batch that fails leaves every one of its articles unconfirmed, and
    /// the resume fetches them again - never a part of the batch counted and the rest lost.
    pub async fn checkpoint_nzb_assembly_segments(
        &self,
        file_id: rd_core::NzbFileId,
        name: String,
        declared_size: u64,
        segments: Vec<crate::AssembledSegment>,
    ) -> Result<()> {
        writer::request(&self.writer, |reply| NzbCommand::CheckpointNzb {
            checkpoint: crate::postprocess_store::NzbCheckpoint::AssemblySegments {
                file_id,
                name,
                declared_size,
                segments,
            },
            reply,
        })
        .await
    }
}
