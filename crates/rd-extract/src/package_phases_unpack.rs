//! The unpack phase of a package's pipeline: the archive sets, then (for a package that
//! unpacks recursively) the archives found inside them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::PostprocessStep;
use rd_postprocess::group_archive_sets;

use super::ArchiveTools;
use crate::{Inner, cleanup_job, package_job::Run, settings, unpack_job};

/// Unpacks the package's archive sets, and what they turn out to contain when the package
/// unpacks recursively.
///
/// `adopt_direct`: a set unpacked while the package downloaded is moved into place instead of
/// being unpacked again (RD-1100-07).
pub(crate) async fn unpack(
    run: &Run<'_>,
    steps: &[PostprocessStep],
    tools: &ArchiveTools,
    adopt_direct: bool,
) -> Result<bool> {
    let inner = run.inner;
    let rar_choice = &tools.rar_choice;
    let target = if run.chosen.unpack_to_subfolder {
        unpack_job::UnpackTarget::OwnFolder
    } else {
        unpack_job::UnpackTarget::Package
    };
    let context = unpack_job::UnpackContext {
        owner: &run.owner,
        directory: &run.directory,
        downloads: &run.downloads,
        candidates: &tools.candidates,
        limits: settings::archive_limits(&run.settings),
        rar_tool: rar_choice.tool.clone(),
        rar_conflict: rar_choice.conflict.clone(),
        rar_outdated: rar_choice.outdated.clone(),
        delete_volumes: run.chosen.level.deletes(),
        trigger: run.trigger,
        target,
        direct: if adopt_direct {
            run.direct.as_slice()
        } else {
            &[]
        },
    };
    let mut unpack_ok = unpack_job::run(inner, &context, steps, &run.sets).await?;
    // Never for torrents (see `PackageSettings::recursive_unpack`).
    if run.chosen.recursive_unpack && unpack_ok {
        unpack_ok = unpack_nested(inner, &context, steps, &run.sets, &run.chosen.rules).await?;
    }
    Ok(unpack_ok)
}

/// Passes of nested extraction after the initial one; caps zip-bomb style chains.
const MAX_RECURSIVE_UNPACK_DEPTH: usize = 3;

/// Extracts archives found inside just-extracted archives, re-scanning the package folder
/// (a real filesystem walk — inner archives have no download rows) after every pass. With a
/// folder per archive, an inner archive stays inside the folder its outer one went into.
/// Inner archives are intermediates and are always deleted after a successful extraction,
/// independent of the package's delete level. Note the `ArchiveLimits` apply per extraction,
/// so the effective ceiling multiplies with the depth cap.
async fn unpack_nested(
    inner: &Inner,
    outer: &unpack_job::UnpackContext<'_>,
    steps: &[rd_core::PostprocessStep],
    initial_sets: &[rd_postprocess::ArchiveSet],
    rules: &cleanup_job::CleanupRules,
) -> Result<bool> {
    // The first volume identifies a set; every initial set is already handled
    // (extracted, skipped or failed), so only genuinely new sets run.
    let mut visited: HashSet<PathBuf> = initial_sets
        .iter()
        .map(|set| set.first().to_path_buf())
        .collect();
    let context = unpack_job::UnpackContext {
        owner: outer.owner,
        directory: outer.directory,
        downloads: outer.downloads,
        candidates: outer.candidates,
        limits: outer.limits,
        rar_tool: outer.rar_tool.clone(),
        rar_conflict: outer.rar_conflict.clone(),
        rar_outdated: outer.rar_outdated.clone(),
        delete_volumes: true,
        trigger: outer.trigger,
        // Nothing nested was unpacked while downloading.
        direct: &[],
        target: match outer.target {
            unpack_job::UnpackTarget::Package => unpack_job::UnpackTarget::Package,
            unpack_job::UnpackTarget::OwnFolder | unpack_job::UnpackTarget::EnclosingFolder => {
                unpack_job::UnpackTarget::EnclosingFolder
            }
        },
    };
    for _ in 0..MAX_RECURSIVE_UNPACK_DEPTH {
        let directory = outer.directory.to_path_buf();
        let rules = rules.clone();
        // The walk and the sizes are file system calls, kept off the async workers.
        let files = tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>> {
            let mut files = Vec::new();
            collect_files(&directory, &mut files)?;
            files.retain(|path| {
                !std::fs::metadata(path).is_ok_and(|meta| rules.is_sample(path, meta.len()))
            });
            Ok(files)
        })
        .await??;
        let new_sets: Vec<rd_postprocess::ArchiveSet> = group_archive_sets(&files)
            .into_iter()
            .filter(|set| !visited.contains(set.first()))
            .collect();
        if new_sets.is_empty() {
            return Ok(true);
        }
        for set in &new_sets {
            visited.insert(set.first().to_path_buf());
        }
        if !unpack_job::run(inner, &context, steps, &new_sets).await? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Recursive walk collecting regular files (extracted content may live in subdirectories).
fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(&entry.path(), files)?;
        } else if file_type.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}
