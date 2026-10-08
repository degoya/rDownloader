//! Orchestrates the post-processing pipeline of one package:
//! PAR2 (Usenet) → SFV → unpack → delete archives → cleanup → dissolve a folder named like the
//! package → malware scan → plugin steps → script → upload → sort.
//!
//! [`run_package`] holds the order and the decisions between the phases; what each phase does
//! is in `package_phases`, and what the package runs with is resolved in `package_settings`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::{DownloadFile, PackageId, PackageState, PostprocessSettings, PostprocessStep};
use rd_postprocess::ArchiveSet;

use crate::{
    ExtractionTrigger, Inner, direct_unpack,
    package_phases::{self, ArchiveTools, Verification},
    package_settings::PackageSettings,
    par2_job,
    pipeline::PlannedStep,
    steps::{StepEnd, find_step},
};

#[path = "package_job_prepare.rs"]
mod prepare;

use prepare::prepare;

/// One package's pass through the pipeline: the package, what it runs with and what it holds.
pub(crate) struct Run<'a> {
    pub(crate) inner: &'a Inner,
    pub(crate) package_id: PackageId,
    pub(crate) trigger: ExtractionTrigger,
    /// A stop interrupted this package's previous run, and what that run finished stays
    /// finished (RA-IN-01).
    pub(crate) resuming: bool,
    pub(crate) package: rd_core::DownloadPackage,
    /// The name of the package's category, for the user script.
    pub(crate) category: Option<String>,
    pub(crate) downloads: Vec<DownloadFile>,
    pub(crate) settings: PostprocessSettings,
    pub(crate) chosen: PackageSettings,
    pub(crate) directory: PathBuf,
    /// The package id as the step rows' owner.
    pub(crate) owner: String,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) sets: Vec<ArchiveSet>,
    pub(crate) sfv_indexes: Vec<PathBuf>,
    pub(crate) plan: Vec<PlannedStep>,
    pub(crate) download_failed: bool,
    /// The sets unpacked while the package downloaded (RD-1100-07), until the unpack adopts
    /// them or they are discarded.
    pub(crate) direct: Vec<direct_unpack::Staged>,
}

/// Runs the whole pipeline for a package and records the package state.
pub(crate) async fn run_package(
    inner: &Inner,
    package_id: PackageId,
    trigger: ExtractionTrigger,
) -> Result<()> {
    let Some(run) = prepare(inner, package_id, trigger).await? else {
        return Ok(());
    };

    let _hold = inner.hold.acquire();
    let steps = enqueue(&run).await?;
    // What the package holds before anything is unpacked or removed, so a plugin step can be
    // told which files the pipeline took away (RD-190-06).
    let before = if run.chosen.plugin_steps.is_empty() {
        Vec::new()
    } else {
        package_file_names(&run.directory).await
    };
    let tools = package_phases::archive_tools(&run).await?;

    // `None`: the package went back to downloading the recovery volumes it is short of.
    let Some(verification) = package_phases::verify(&run, &steps, &tools).await? else {
        return Ok(());
    };
    let unpacked = unpack_and_clean(&run, &steps, &tools, verification).await?;
    let Some(handed_on) = hand_on(&run, &steps, &before, &unpacked).await? else {
        return Ok(());
    };
    let Unpacked {
        verification_gate,
        unpack_ok,
        ..
    } = unpacked;
    // A package with a file missing is never completed (RD-1190-13); only a manual run gets
    // this far with one.
    let final_state = if verification_gate
        && unpack_ok
        && handed_on.scan_clean
        && handed_on.plugin_steps_ok
        && handed_on.upload_ok
        && !crate::completion::parts_missing(run.package.kind, &run.downloads)
    {
        PackageState::Completed
    } else {
        PackageState::Failed
    };
    let final_state = sort(&run, &steps, final_state).await?;
    record_outcome(&run, verification_gate, unpack_ok, final_state).await
}

/// What the verification and the unpack made of the package.
struct Unpacked {
    verified: bool,
    /// SABnzbd's `safe_postproc` (see `PackageSettings::safe_postproc`).
    verification_gate: bool,
    unpack_ok: bool,
    /// What the cleanup removed.
    cleaned: Vec<String>,
}

/// The unpack, the recovery data's deletion, the cleanup, a recording's remux and a folder named
/// like the package dissolved: the package as it will be kept.
async fn unpack_and_clean(
    run: &Run<'_>,
    steps: &[PostprocessStep],
    tools: &ArchiveTools,
    verification: Verification,
) -> Result<Unpacked> {
    let inner = run.inner;
    let level = run.chosen.level;
    let Verification {
        par2_ok,
        sfv_ok,
        rar_test_ok,
        repaired,
    } = verification;
    let verified = par2_ok && sfv_ok && rar_test_ok;
    // SABnzbd's `safe_postproc` (see `PackageSettings::safe_postproc`).
    let verification_gate = verified || !run.chosen.safe_postproc;

    let mut unpack_ok = true;
    if level.unpacks() && verification_gate && !run.sets.is_empty() {
        // PAR2 wins (RD-1100-07): what was unpacked while the package downloaded is only used
        // when the verification passed without repairing anything. Otherwise the tool may have
        // read a volume the repair has since rewritten, and the set is unpacked again.
        let adopt_direct = verified && !repaired;
        unpack_ok = package_phases::unpack(run, steps, tools, adopt_direct).await?;
    }
    // Whatever was not adopted is gone before anything walks the package.
    direct_unpack::discard(&run.direct).await;
    // Only once everything that could still need the recovery data has succeeded.
    if run.chosen.delete_par2 && level.deletes() && par2_ok && sfv_ok && unpack_ok {
        par2_job::delete_sets(inner, &run.owner, steps, &run.files).await?;
    }
    let mut cleaned = Vec::new();
    if level.unpacks() && verification_gate && unpack_ok && run.chosen.rules.is_active() {
        cleaned = package_phases::clean_up(run).await?;
    }
    // A livestream recording's segments are joined here rather than by the recorder: a
    // six-hour remux has to survive a restart, and a persisted step is what makes that a
    // resumed job instead of a lost one (RD-080-09).
    package_phases::remux_recording(
        inner,
        &run.owner,
        &run.downloads,
        &run.directory,
        &run.settings,
    )
    .await?;
    // Last, so what moves up is what will be kept, and only for a package that got this far: a
    // failed one keeps its layout for the retry (RD-1140-01).
    crate::unwrap_job::run(
        run,
        run.chosen.unwrap_package_folder && verification_gate && unpack_ok,
    )
    .await?;
    Ok(Unpacked {
        verified,
        verification_gate,
        unpack_ok,
        cleaned,
    })
}

/// How the steps that hand the package on ended.
struct HandedOn {
    scan_clean: bool,
    plugin_steps_ok: bool,
    upload_ok: bool,
}

/// The malware scan, the plugin steps, the user script and the upload.
///
/// `None` when a plugin step or the upload was stopped: the package stays in post-processing
/// for the next start.
async fn hand_on(
    run: &Run<'_>,
    steps: &[PostprocessStep],
    before: &[String],
    unpacked: &Unpacked,
) -> Result<Option<HandedOn>> {
    let verification_gate = unpacked.verification_gate;
    let unpack_ok = unpacked.unpack_ok;
    // After cleanup, so what is scanned is what will be kept, and before anything hands the
    // package on — a plugin step, the user script, an upload (RD-190-14). It runs whatever the
    // unpack made of the package: an upload goes ahead after a failed unpack too, and clamd
    // looks inside the archives that are left. A finding stops everything after it.
    let mut scan_clean = true;
    if run.chosen.malware_scan {
        scan_clean = package_phases::scan(run).await?;
    }
    // After cleanup, so a plugin sees the package as it will finally be, and before the user
    // script, which stays the last word. Only for a package that got that far: running a
    // checksum step over a half-unpacked package would report a mismatch that says nothing.
    let mut plugin_steps_ok = true;
    if scan_clean && !run.chosen.plugin_steps.is_empty() && verification_gate && unpack_ok {
        match package_phases::plugin_steps(run, before, &unpacked.cleaned).await? {
            StepEnd::Done => {}
            StepEnd::Failed => plugin_steps_ok = false,
            // Left in post-processing for the next start, as a stopped upload is below.
            StepEnd::Stopped => return Ok(None),
        }
    }
    let upload_end = script_and_upload(run, steps, scan_clean, unpacked).await?;
    // A stop is no verdict: the package stays in post-processing with the upload queued, and
    // the next start resumes it. Marking it completed here dropped its import history and left
    // the restart a queued row to run the whole pipeline for (audit 1.9.1, RA-IN-01).
    if upload_end == StepEnd::Stopped {
        return Ok(None);
    }
    Ok(Some(HandedOn {
        scan_clean,
        plugin_steps_ok,
        upload_ok: upload_end == StepEnd::Done,
    }))
}

/// The user script and the upload, each unless a resumed pass already ran it to an end; says
/// how the upload ended (`Done` when there is none).
async fn script_and_upload(
    run: &Run<'_>,
    steps: &[PostprocessStep],
    scan_clean: bool,
    unpacked: &Unpacked,
) -> Result<StepEnd> {
    // A script that ran to an end, whatever it answered, is not run a second time: what it
    // did outside the package cannot be undone by running it again.
    let script_done = |name: &str| {
        run.resuming
            && find_step(steps, rd_core::PostprocessKind::Script, name).is_some_and(|step| {
                matches!(
                    step.state,
                    rd_core::PostprocessState::Completed | rd_core::PostprocessState::Failed
                )
            })
    };
    if let Some(name) = run
        .chosen
        .script
        .as_ref()
        .filter(|name| scan_clean && !script_done(name.as_str()))
    {
        let status = package_phases::script_status(
            unpacked.verified,
            unpacked.unpack_ok,
            run.download_failed,
        );
        package_phases::script(run, name, status).await?;
    }
    let upload_done = |remote: &str| {
        run.resuming
            && find_step(steps, rd_core::PostprocessKind::Upload, remote)
                .is_some_and(|step| step.state == rd_core::PostprocessState::Completed)
    };
    let mut upload_end = StepEnd::Done;
    if let Some(remote) = run
        .chosen
        .upload
        .as_ref()
        .filter(|remote| scan_clean && !upload_done(remote.as_str()))
    {
        upload_end = package_phases::upload(run, remote).await?;
    }
    Ok(upload_end)
}

/// The sort, last and only for a package that succeeded; the package's final state after it.
async fn sort(
    run: &Run<'_>,
    steps: &[PostprocessStep],
    mut final_state: PackageState,
) -> Result<PackageState> {
    // Last, and only for a package that succeeded (RD-1100-08): a failed one keeps its files
    // where a retry finds them. A sort that finished is not run again on a resumed pass — what it
    // moved is no longer in the package anyway, so a second run would only report nothing.
    if let Some(templates) = run.chosen.sorting.as_ref() {
        let sort_done = run.resuming
            && find_step(
                steps,
                rd_core::PostprocessKind::Sort,
                crate::sort_job::SOURCE,
            )
            .is_some_and(|step| step.state == rd_core::PostprocessState::Completed);
        if final_state != PackageState::Completed {
            crate::sort_job::skip(run.inner, &run.owner).await?;
        } else if !sort_done && crate::sort_job::run(run, templates).await? != StepEnd::Done {
            final_state = PackageState::Failed;
        }
    }
    Ok(final_state)
}

/// Queues the planned steps, moves the package into post-processing and returns its steps.
async fn enqueue(run: &Run<'_>) -> Result<Vec<rd_core::PostprocessStep>> {
    let inner = run.inner;
    inner
        .database
        .enqueue_postprocess_steps(
            run.owner.clone(),
            run.plan
                .iter()
                .map(|step| (step.kind, step.source.clone(), step.position))
                .collect(),
        )
        .await?;
    inner
        .database
        .set_package_state(
            run.package_id,
            PackageState::Postprocessing,
            None,
            None,
            None,
        )
        .await?;
    inner.database.list_postprocess_steps(&run.owner).await
}

/// Records the extraction outcome and the package's final state.
async fn record_outcome(
    run: &Run<'_>,
    verification_gate: bool,
    unpack_ok: bool,
    final_state: PackageState,
) -> Result<()> {
    let inner = run.inner;
    // Written before the state so the refresh triggered by `package.state` sees it.
    if run.chosen.level.unpacks() && verification_gate && !run.sets.is_empty() {
        let outcome = if unpack_ok {
            rd_core::ExtractionResult::Success
        } else {
            rd_core::ExtractionResult::Failed
        };
        inner
            .database
            .set_package_extraction(run.package_id, Some(outcome))
            .await?;
    }
    inner
        .database
        .set_package_state(run.package_id, final_state, None, None, None)
        .await?;
    if final_state == PackageState::Completed {
        forget_import_history(inner, &run.package, &run.settings).await;
    }
    Ok(())
}

/// Drops the package's NZB import history after a successful completion when the user
/// opted out of keeping it. Failures only log: the download itself already succeeded.
async fn forget_import_history(
    inner: &Inner,
    package: &rd_core::DownloadPackage,
    settings: &PostprocessSettings,
) {
    if settings.keep_import_history || package.kind != rd_core::DownloadKind::Usenet {
        return;
    }
    if let Err(error) = inner.database.forget_nzb_import_history(package.id).await {
        tracing::warn!(package_id = %package.id, %error, "dropping NZB import history failed");
    }
}

/// The files of a package directory, by path relative to it with `/` between folders, as a
/// plugin step or an upload may see them.
///
/// Built here rather than reusing the planner's list: that one holds absolute paths, and what
/// a step is offered are names relative to its package — it never learns where the package is.
/// Folders are walked, because unpacked content can sit in a folder per archive (RD-170-16) or
/// in the archive's own folders; symbolic links are neither followed nor listed, and a staging
/// folder an interrupted unpack left behind is skipped. Sorted, so a resumed step sees the same
/// order.
pub(crate) async fn package_file_names(directory: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let mut pending = vec![(directory.to_path_buf(), String::new())];
    while let Some((folder, prefix)) = pending.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&folder).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            // `DirEntry::file_type` does not follow a link, so a link is neither kind.
            let Ok(kind) = entry.file_type().await else {
                continue;
            };
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if kind.is_file() {
                names.push(relative);
            } else if kind.is_dir() && !name.starts_with(rd_postprocess::STAGING_PREFIX) {
                pending.push((entry.path(), relative));
            }
        }
    }
    names.sort();
    names
}
