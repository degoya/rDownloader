//! Pure planner: which post-processing steps run for a package, in which order.

use std::path::PathBuf;

use rd_core::{DownloadKind, PostprocessKind, PostprocessLevel, PostprocessStage};
use rd_postprocess::{ArchiveSet, is_main_par2};

/// One planned step with its stable pipeline position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlannedStep {
    pub kind: PostprocessKind,
    pub source: String,
    pub position: i64,
    pub stage: PostprocessStage,
}

/// Inputs that decide the pipeline.
pub(crate) struct PlanInput<'a> {
    pub level: PostprocessLevel,
    pub kind: DownloadKind,
    pub files: &'a [PathBuf],
    pub sets: &'a [ArchiveSet],
    /// `.sfv` indexes to verify; empty when the effective setting switches the check off.
    pub sfv: &'a [PathBuf],
    pub script: Option<&'a str>,
    /// Whether the cleanup step is worth scheduling (extensions or samples configured).
    pub cleanup_enabled: bool,
    /// rclone target (`remote:path`) when the effective upload settings are enabled.
    pub upload: Option<&'a str>,
    /// Whether the PAR2 recovery set is removed once unpacking has succeeded.
    pub delete_par2: bool,
    /// Installed plugin steps to run, by plugin id and in the order they should run.
    pub plugin_steps: &'a [String],
    /// Whether the package is scanned for malware (RD-190-14).
    pub malware_scan: bool,
    /// Whether the category sorts series and films (RD-1100-08).
    pub sort: bool,
}

/// Order: PAR2 (Usenet only) → SFV → unpack → delete archives → delete PAR2 → cleanup →
/// malware scan → plugin steps → script → upload → sort.
///
/// Plugin steps come after cleanup and before scripts: by then the package is what it will
/// finally be — unpacked and tidied — and a user script stays the last word, as it was. The
/// malware scan comes right before them (RD-190-14): what it scans is what will be kept, and
/// nothing hands the package on before it has had its say. It runs at every level, `None`
/// included — whether a package is scanned is its own switch, not a degree of unpacking.
///
/// The sort comes last (RD-1100-08): it moves files out of the package folder, so everything
/// that works on the package — the script, the upload — has seen it whole first. Like the scan
/// it runs at every level: a single video needs no unpacking to be placed.
pub(crate) fn plan(input: &PlanInput<'_>) -> Vec<PlannedStep> {
    let mut steps = Vec::new();
    let mut position = 0_i64;
    let mut push = |kind, source: String, stage| {
        position += 1;
        steps.push(PlannedStep {
            kind,
            source,
            position,
            stage,
        });
    };
    if input.level.repairs() && input.kind == DownloadKind::Usenet {
        for index in input.files.iter().filter(|path| is_main_par2(path)) {
            push(
                PostprocessKind::Par2,
                index.to_string_lossy().into_owned(),
                PostprocessStage::Repairing,
            );
        }
    }
    // Unlike PAR2 this runs for every kind, and before unpacking: at `+Delete` the volumes
    // an index lists are gone once extraction succeeded. `None` means no post-processing at
    // all, so verification starts at `+Repair` like the other integrity step.
    if input.level.repairs() {
        for index in input.sfv {
            push(
                PostprocessKind::Sfv,
                index.to_string_lossy().into_owned(),
                PostprocessStage::Verifying,
            );
        }
        // The substitute check, planned unconditionally so it is visible either way: it runs
        // when neither PAR2 nor an `.sfv` index answered, and is recorded as skipped — with
        // the reason — when one of them did (RD-104-04).
        for set in input
            .sets
            .iter()
            .filter(|set| set.kind == rd_files::ArchiveKind::Rar)
        {
            push(
                PostprocessKind::RarTest,
                set.first().to_string_lossy().into_owned(),
                PostprocessStage::Verifying,
            );
        }
    }
    if input.level.unpacks() {
        for set in input.sets {
            let source = set.first().to_string_lossy().into_owned();
            push(
                archive_kind(set),
                source.clone(),
                PostprocessStage::Extracting,
            );
            if input.level.deletes() {
                push(
                    PostprocessKind::DeleteArchives,
                    source,
                    PostprocessStage::DeletingArchives,
                );
            }
        }
        // After unpacking, never straight after the repair. PAR2 runs first, so deleting the
        // recovery set at repair time would leave a package whose extraction then failed with
        // no way to try again. Removing it here means it survives exactly as long as it could
        // still be needed.
        if input.delete_par2 && input.level.deletes() && input.kind == DownloadKind::Usenet {
            for index in input.files.iter().filter(|path| is_main_par2(path)) {
                push(
                    PostprocessKind::DeletePar2,
                    index.to_string_lossy().into_owned(),
                    PostprocessStage::DeletingPar2,
                );
            }
        }
        if input.cleanup_enabled {
            push(
                PostprocessKind::Cleanup,
                "cleanup".to_owned(),
                PostprocessStage::Cleaning,
            );
        }
    }
    if input.malware_scan {
        push(
            PostprocessKind::MalwareScan,
            crate::malware_scan::SCANNER.to_owned(),
            PostprocessStage::Scanning,
        );
    }
    for plugin_id in input.plugin_steps {
        push(
            PostprocessKind::PluginStep,
            plugin_id.clone(),
            PostprocessStage::PluginStep,
        );
    }
    if let Some(script) = input.script {
        push(
            PostprocessKind::Script,
            script.to_owned(),
            PostprocessStage::Script,
        );
    }
    if let Some(remote) = input.upload {
        push(
            PostprocessKind::Upload,
            remote.to_owned(),
            PostprocessStage::Uploading,
        );
    }
    if input.sort {
        push(
            PostprocessKind::Sort,
            crate::sort_job::SOURCE.to_owned(),
            PostprocessStage::Sorting,
        );
    }
    steps
}

pub(crate) const fn archive_kind(set: &ArchiveSet) -> PostprocessKind {
    match set.kind {
        rd_files::ArchiveKind::Zip => PostprocessKind::ExtractZip,
        rd_files::ArchiveKind::SevenZip => PostprocessKind::ExtractSevenZip,
        rd_files::ArchiveKind::Rar => PostprocessKind::ExtractRar,
    }
}

#[cfg(test)]
mod tests;
