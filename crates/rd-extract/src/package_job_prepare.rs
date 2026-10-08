//! Getting one package ready for its pass through the pipeline: loading it, resolving what it
//! runs with, listing what it holds and planning its steps.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rd_core::{DownloadFile, DownloadState, PackageId, PackageState};
use rd_postprocess::{group_archive_sets, is_sfv};

use super::{Run, forget_import_history};
use crate::{
    ExtractionTrigger, Inner, direct_unpack,
    package_settings::PackageSettings,
    par2_refill,
    pipeline::{self, PlanInput},
    settings,
};

/// Loads the package, resolves what it runs with and plans its steps.
///
/// `None` when nothing runs: the package is waiting for recovery volumes, or its plan is
/// empty and it was recorded as completed.
pub(super) async fn prepare(
    inner: &Inner,
    package_id: PackageId,
    trigger: ExtractionTrigger,
) -> Result<Option<Run<'_>>> {
    let package = inner
        .database
        .get_package(package_id)
        .await?
        .context("package not found")?;
    // Nothing runs at start-up, so a package found in post-processing by an automatic run is
    // one a stop interrupted and `recover()` asked for again: what that run finished stays
    // finished, the user script and the upload included (audit 1.9.1, RA-IN-01). A manual run
    // is never this — the interface refuses one while the package is in post-processing.
    let resuming =
        trigger == ExtractionTrigger::Auto && package.state == PackageState::Postprocessing;
    let downloads: Vec<DownloadFile> = inner.database.downloads_for_package(package_id).await?;
    // A package whose postponed recovery volumes are still on their way is not ready for the
    // pipeline (RD-107-04). Verifying now would measure the same gap a second time and plan
    // against volumes that are already coming; the completion listener requests the package
    // again once the last of them lands. "Post-process anyway" is the exception: somebody who
    // asked for that has decided not to wait for a verdict at all.
    let waiting_for_volumes = package.kind == rd_core::DownloadKind::Usenet
        && trigger != ExtractionTrigger::Force
        && par2_refill::volumes_on_the_way(&downloads);
    if waiting_for_volumes {
        inner
            .database
            .set_package_state(package_id, PackageState::Downloading, None, None, None)
            .await?;
        return Ok(None);
    }
    // What was unpacked while the package downloaded; a manual run stops what is still running
    // and unpacks the normal way (RD-1100-07). Every other direct staging directory in the
    // folder is a leftover — of a stop, a restart or such a manual run — and goes now.
    let direct = inner
        .direct
        .settle(package_id, trigger == ExtractionTrigger::Auto)
        .await;
    direct_unpack::sweep(Path::new(&package.destination), &direct).await;
    let settings = settings::load_postprocess_settings(&inner.database).await?;
    let categories = inner.database.list_categories().await?;
    let category = package
        .category_id
        .and_then(|id| categories.iter().find(|category| category.id == id));
    let chosen = PackageSettings::resolve(inner, &package, category, &settings, trigger);
    let directory = PathBuf::from(&package.destination);
    let finished: Vec<PathBuf> = downloads
        .iter()
        .filter(|file| {
            matches!(
                file.state,
                DownloadState::Completed | DownloadState::Seeding
            )
        })
        .map(|file| directory.join(&file.file_name))
        .collect();
    let rules = chosen.rules.clone();
    let files = tokio::task::spawn_blocking(move || rules.present_without_samples(finished))
        .await
        .context("join the package's file listing")?;
    let sets = group_archive_sets(&files);
    let sfv_indexes: Vec<PathBuf> = if chosen.sfv_verify {
        files.iter().filter(|path| is_sfv(path)).cloned().collect()
    } else {
        Vec::new()
    };
    let owner = package_id.to_string();
    let plan = pipeline::plan(&PlanInput {
        level: chosen.level,
        kind: package.kind,
        files: &files,
        sets: &sets,
        sfv: &sfv_indexes,
        script: chosen.script.as_deref(),
        cleanup_enabled: chosen.rules.is_active(),
        upload: chosen.upload.as_deref(),
        delete_par2: chosen.delete_par2,
        plugin_steps: &chosen.plugin_steps,
        malware_scan: chosen.malware_scan,
        sort: chosen.sorting.is_some(),
    });
    let download_failed = downloads.iter().any(|file| {
        !matches!(
            file.state,
            DownloadState::Completed
                | DownloadState::Cancelled
                // A mirror that stood down did not fail: the file it stood in for arrived
                // through another link. Reporting status 1 would tell a user script the
                // package is broken when nothing about it is.
                | DownloadState::Skipped
        )
    });
    // A fresh run invalidates the outcome of any previous one.
    inner
        .database
        .set_package_extraction(package_id, None)
        .await?;
    if plan.is_empty() {
        direct_unpack::discard(&direct).await;
        // Nothing to do is no success while a file is missing (RD-1190-13): only a manual run
        // gets here with one, and the package then reads as failed, not as finished.
        let state = if crate::completion::parts_missing(package.kind, &downloads) {
            PackageState::Failed
        } else {
            PackageState::Completed
        };
        inner
            .database
            .set_package_state(package_id, state, None, None, None)
            .await?;
        if state == PackageState::Completed {
            forget_import_history(inner, &package, &settings).await;
        }
        return Ok(None);
    }
    let category = category.map(|category| category.name.clone());
    Ok(Some(Run {
        inner,
        package_id,
        trigger,
        resuming,
        package,
        category,
        downloads,
        settings,
        chosen,
        directory,
        owner,
        files,
        sets,
        sfv_indexes,
        plan,
        download_failed,
        direct,
    }))
}
