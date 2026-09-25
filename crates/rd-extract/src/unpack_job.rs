//! Archive extraction into the package folder plus optional deletion of the volumes.

use std::path::Path;

use anyhow::Result;
use rd_core::{DownloadFile, DownloadState, PostprocessKind, PostprocessState, PostprocessStep};
use rd_postprocess::{
    ArchiveLimits, ArchiveSet, ExternalRarTool, ExtractRequest, extract_with_passwords,
};

use crate::{
    ExtractionTrigger, Inner,
    pipeline::archive_kind,
    steps::{
        Outcome, checkpoint, checkpoint_coded, drain_progress, extraction_outcome, find_step,
        path_string,
    },
};

/// No external RAR/7z tool is configured, so the question could not even be asked.
///
/// Not an [`rd_postprocess::ExtractionError`]: nothing failed to extract, and the step is
/// recorded as skipped. It is a step code all the same, translated like the rest.
const NO_TOOL: &str = "extract.no_tool";

pub(crate) struct UnpackContext<'a> {
    pub owner: &'a str,
    pub directory: &'a Path,
    pub downloads: &'a [DownloadFile],
    pub candidates: &'a [Option<String>],
    pub limits: ArchiveLimits,
    pub rar_tool: Option<ExternalRarTool>,
    /// Why there is no tool, when the two RAR settings contradict each other (RD-107-11).
    pub rar_conflict: Option<String>,
    pub delete_volumes: bool,
    pub trigger: ExtractionTrigger,
}

/// Extracts every set; returns `false` when any set failed.
pub(crate) async fn run(
    inner: &Inner,
    context: &UnpackContext<'_>,
    steps: &[PostprocessStep],
    sets: &[ArchiveSet],
) -> Result<bool> {
    let mut ok = true;
    for set in sets {
        let source = path_string(set.first())?;
        let kind = archive_kind(set);
        let existing = find_step(steps, kind, &source);
        // Archives are unpacked straight into the package folder (merged, replacing
        // same-named entries); a manual run therefore always re-extracts.
        if existing.is_some_and(|step| step.state == PostprocessState::Completed)
            && context.trigger == ExtractionTrigger::Auto
        {
            continue;
        }
        let needs_tool = set.kind == rd_files::ArchiveKind::Rar
            || (set.kind == rd_files::ArchiveKind::Zip && set.is_multipart());
        if needs_tool && context.rar_tool.is_none() {
            // A missing tool is a question that could not be asked; a contradiction between
            // `rar_tool` and `rar_executable` is a misconfiguration, and saying so is the whole
            // point of detecting it (RD-107-11).
            let (state, outcome) = context.rar_conflict.as_ref().map_or_else(
                || {
                    (
                        PostprocessState::Skipped,
                        Outcome {
                            code: NO_TOOL,
                            params: rd_core::MessageParams::new(),
                            message: Some("no external RAR/7z tool is configured".to_owned()),
                        },
                    )
                },
                |conflict| {
                    (
                        PostprocessState::Failed,
                        extraction_outcome(&rd_postprocess::ExtractionError::ToolMismatch(
                            conflict.clone(),
                        )),
                    )
                },
            );
            if state == PostprocessState::Failed {
                ok = false;
            }
            checkpoint_coded(inner, context.owner, kind, &source, state, None, outcome).await?;
            continue;
        }
        crate::steps::stage(
            inner,
            context.owner,
            rd_core::PostprocessStage::Extracting,
            set.first()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
        )
        .await?;
        let destination_text = path_string(context.directory)?;
        let volume_ids: Vec<_> = context
            .downloads
            .iter()
            .filter(|file| {
                set.volumes
                    .iter()
                    .any(|volume| *volume == context.directory.join(&file.file_name))
            })
            .map(|file| file.id)
            .collect();
        for id in &volume_ids {
            let _ = inner
                .database
                .transition_download(*id, DownloadState::Extracting)
                .await;
        }
        checkpoint(
            inner,
            context.owner,
            kind,
            &source,
            PostprocessState::Running,
            Some(destination_text.clone()),
            None,
        )
        .await?;
        let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let drain = tokio::spawn(drain_progress(
            inner.database.clone(),
            context.owner.to_owned(),
            kind,
            source.clone(),
            rd_core::PostprocessStage::Extracting,
            progress_rx,
        ));
        let result = extract_with_passwords(
            ExtractRequest {
                set,
                destination: context.directory.to_owned(),
                limits: context.limits,
                rar_tool: context.rar_tool.clone(),
                merge: true,
                progress: Some(progress_tx),
            },
            context.candidates,
        )
        .await;
        let _ = drain.await;
        for id in &volume_ids {
            let _ = inner
                .database
                .transition_download(*id, DownloadState::Completed)
                .await;
        }
        match result {
            Ok((report, _)) => {
                checkpoint(
                    inner,
                    context.owner,
                    kind,
                    &source,
                    PostprocessState::Completed,
                    Some(destination_text),
                    Some(format!(
                        "files={} bytes={}",
                        report.files, report.uncompressed_bytes
                    )),
                )
                .await?;
                if context.delete_volumes {
                    delete_volumes(inner, context.owner, set).await?;
                }
            }
            Err(error) => {
                ok = false;
                checkpoint_coded(
                    inner,
                    context.owner,
                    kind,
                    &source,
                    PostprocessState::Failed,
                    Some(destination_text),
                    extraction_outcome(&error),
                )
                .await?;
            }
        }
    }
    Ok(ok)
}

async fn delete_volumes(inner: &Inner, owner: &str, set: &ArchiveSet) -> Result<()> {
    let source = path_string(set.first())?;
    crate::steps::stage(
        inner,
        owner,
        rd_core::PostprocessStage::DeletingArchives,
        None,
    )
    .await?;
    checkpoint(
        inner,
        owner,
        PostprocessKind::DeleteArchives,
        &source,
        PostprocessState::Running,
        None,
        None,
    )
    .await?;
    for volume in &set.volumes {
        if let Err(error) = tokio::fs::remove_file(volume).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            checkpoint(
                inner,
                owner,
                PostprocessKind::DeleteArchives,
                &source,
                PostprocessState::Failed,
                None,
                Some(format!("{}: {error}", volume.display())),
            )
            .await?;
            return Ok(());
        }
    }
    checkpoint(
        inner,
        owner,
        PostprocessKind::DeleteArchives,
        &source,
        PostprocessState::Completed,
        None,
        Some(format!("removed={}", set.volumes.len())),
    )
    .await
}
