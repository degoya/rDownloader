//! Database facade for post-processing checkpoints and steps, package state and extraction.

use anyhow::Result;

use crate::{
    Database,
    archive_password::PasswordTable,
    commands::{ConfigCommand, NzbCommand, PackagesCommand},
    writer,
};

impl Database {
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
        writer::request(&self.writer, |reply| NzbCommand::CheckpointNzb {
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
        writer::request(&self.writer, |reply| NzbCommand::CheckpointNzb {
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
        writer::request(&self.writer, |reply| NzbCommand::CheckpointNzb {
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
        writer::request(&self.writer, |reply| NzbCommand::CheckpointNzb {
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
        writer::request(&self.writer, |reply| PackagesCommand::SetPackageState {
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
        writer::request(&self.writer, |reply| {
            PackagesCommand::SetPackageExtraction { id, result, reply }
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
            ConfigCommand::UpdateCategoryPostprocess {
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

    /// Archive password of a package, read from the vault (RD-190-04); for the extraction.
    pub async fn package_password(&self, id: rd_core::PackageId) -> Result<Option<String>> {
        self.archive_password(PasswordTable::Packages, id.to_string())
            .await
    }
}
