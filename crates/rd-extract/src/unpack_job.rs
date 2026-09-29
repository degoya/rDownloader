//! Archive extraction into the package folder, or a folder per archive, plus optional deletion
//! of the volumes.

use std::path::{Path, PathBuf};

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

/// The tool that was found is older than its security floor (security review 2026-09-28,
/// finding 5), with `{tool}`, `{found}` and `{minimum}`.
pub(crate) const TOOL_OUTDATED: &str = "extract.tool_outdated";

pub(crate) struct UnpackContext<'a> {
    pub owner: &'a str,
    pub directory: &'a Path,
    pub downloads: &'a [DownloadFile],
    pub candidates: &'a [Option<String>],
    pub limits: ArchiveLimits,
    pub rar_tool: Option<ExternalRarTool>,
    /// Why there is no tool, when the two RAR settings contradict each other (RD-107-11).
    pub rar_conflict: Option<String>,
    /// Why there is no tool, when the one found is below its security floor.
    pub rar_outdated: Option<crate::settings::OutdatedTool>,
    pub delete_volumes: bool,
    pub trigger: ExtractionTrigger,
    pub target: UnpackTarget,
}

/// Where a set is unpacked to (RD-170-16).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnpackTarget {
    /// Straight into the package folder, merged with what is there — the default.
    Package,
    /// Into a folder of its own below the package folder, named after the archive
    /// (`Film.part1.rar` → `Film/`). The archives themselves stay where they are.
    OwnFolder,
    /// A nested pass under [`Self::OwnFolder`]: an archive that came out of an archive is
    /// unpacked inside the folder its parent went into, never beside it in the package root.
    EnclosingFolder,
}

impl UnpackTarget {
    /// The directory `set` is unpacked into.
    pub(crate) fn destination(self, directory: &Path, set: &ArchiveSet) -> PathBuf {
        match self {
            Self::Package => directory.to_owned(),
            Self::OwnFolder => rd_files::extraction_subfolder(directory, &set.base),
            Self::EnclosingFolder => set
                .first()
                .strip_prefix(directory)
                .ok()
                .and_then(|relative| {
                    let mut components = relative.components();
                    let top = components.next()?;
                    // Only a set below a folder has one to stay in; the rest has a name.
                    components.next()?;
                    Some(directory.join(top))
                })
                .unwrap_or_else(|| rd_files::extraction_subfolder(directory, &set.base)),
        }
    }
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
        // Archives are unpacked into an existing folder (merged, replacing same-named
        // entries); a manual run therefore always re-extracts.
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
            // point of detecting it (RD-107-11). An outdated tool is refused, not missing.
            let (state, outcome) = match (&context.rar_conflict, &context.rar_outdated) {
                (Some(conflict), _) => (
                    PostprocessState::Failed,
                    extraction_outcome(&rd_postprocess::ExtractionError::ToolMismatch(
                        conflict.clone(),
                    )),
                ),
                (None, Some(outdated)) => (PostprocessState::Failed, outdated_outcome(outdated)),
                (None, None) => (
                    PostprocessState::Skipped,
                    Outcome {
                        code: NO_TOOL,
                        params: rd_core::MessageParams::new(),
                        message: Some("no external RAR/7z tool is configured".to_owned()),
                    },
                ),
            };
            if state == PostprocessState::Failed {
                ok = false;
            }
            checkpoint_coded(inner, context.owner, kind, &source, state, None, outcome).await?;
            continue;
        }
        let destination = context.target.destination(context.directory, set);
        // A folder of its own has to exist before the merge can move anything into it. An
        // existing one is kept: a rerun after a crash finishes what the first run began.
        if context.target != UnpackTarget::Package
            && let Err(error) = tokio::fs::create_dir_all(rd_files::long_path(&destination)).await
        {
            ok = false;
            let error = rd_postprocess::ExtractionError::Other(anyhow::Error::new(error).context(
                format!("create extraction folder {}", destination.display()),
            ));
            checkpoint_coded(
                inner,
                context.owner,
                kind,
                &source,
                PostprocessState::Failed,
                Some(path_string(&destination)?),
                extraction_outcome(&error),
            )
            .await?;
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
        let destination_text = path_string(&destination)?;
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
                destination,
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

/// The step outcome for a tool refused for its version: the code with the tool, the version it
/// reported and the floor, so the interface can say which update is missing.
fn outdated_outcome(outdated: &crate::settings::OutdatedTool) -> Outcome<'static> {
    Outcome {
        code: TOOL_OUTDATED,
        params: [
            ("tool", outdated.tool.to_owned()),
            ("found", outdated.found.clone()),
            ("minimum", outdated.minimum.clone()),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect(),
        message: Some(format!(
            "{} {} is below the version extraction needs ({} or newer)",
            outdated.tool, outdated.found, outdated.minimum
        )),
    }
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
