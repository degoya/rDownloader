//! Orchestrates the post-processing pipeline of one package:
//! PAR2 (Usenet) → SFV → unpack → delete archives → cleanup → script.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rd_core::{
    DownloadFile, DownloadState, PackageId, PackageState, PostprocessLevel, PostprocessSettings,
};
use rd_postprocess::{group_archive_sets, is_sfv, load_password_file, password_candidates};

use crate::{
    ExtractionTrigger, Inner, cleanup_job, par2_job, par2_refill,
    pipeline::{self, PlanInput},
    plugin_step, rar_test_job, rclone_job, script_job, settings, sfv_job, storage_upload,
    unpack_job,
};

/// Runs the whole pipeline for a package and records the package state.
pub(crate) async fn run_package(
    inner: &Inner,
    package_id: PackageId,
    trigger: ExtractionTrigger,
) -> Result<()> {
    let package = inner
        .database
        .list_packages()
        .await?
        .into_iter()
        .find(|package| package.id == package_id)
        .context("package not found")?;
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
        return Ok(());
    }
    let settings = settings::load_postprocess_settings(&inner.database).await?;
    let categories = inner.database.list_categories().await?;
    let category = package
        .category_id
        .and_then(|id| categories.iter().find(|category| category.id == id));
    let mut level = package
        .postprocess_level
        .or(category.and_then(|category| category.postprocess_level))
        .unwrap_or_else(|| settings.effective_default_level());
    if trigger.is_manual() && level < PostprocessLevel::Unpack {
        level = PostprocessLevel::Unpack;
    }
    // A torrent payload may still be seeding; deleting archive volumes would corrupt it.
    if package.kind == rd_core::DownloadKind::Torrent {
        level = level.min(PostprocessLevel::Unpack);
    }
    let script = package
        .script
        .clone()
        .or(category.and_then(|category| category.script.clone()))
        .filter(|name| !name.trim().is_empty());
    let directory = PathBuf::from(&package.destination);
    let rules = cleanup_job::CleanupRules {
        // Same precedence as the level and script above: the category overrides the global
        // list, and an empty override switches cleanup off for that category.
        extensions: category
            .and_then(|category| category.cleanup_extensions.clone())
            .unwrap_or_else(|| settings.cleanup_extensions.clone()),
        ignore_samples: settings.ignore_samples,
        sample_max_bytes: settings.sample_max_bytes.get(),
    };
    let files: Vec<PathBuf> = downloads
        .iter()
        .filter(|file| {
            matches!(
                file.state,
                DownloadState::Completed | DownloadState::Seeding
            )
        })
        .map(|file| directory.join(&file.file_name))
        .filter(|path| path.is_file())
        .filter(|path| !std::fs::metadata(path).is_ok_and(|meta| rules.is_sample(path, meta.len())))
        .collect();
    let sets = group_archive_sets(&files);
    // Same precedence as level, script and cleanup: category override, else the global setting.
    let sfv_indexes: Vec<PathBuf> = if category
        .and_then(|category| category.sfv_verify)
        .unwrap_or(settings.sfv_verify)
    {
        files.iter().filter(|path| is_sfv(path)).cloned().collect()
    } else {
        Vec::new()
    };
    let owner = package_id.to_string();
    // Upload target with the usual precedence: category override, else the global setting.
    let upload_enabled = category
        .and_then(|category| category.upload_enabled)
        .unwrap_or(settings.upload_enabled);
    let upload = upload_enabled
        .then(|| {
            category
                .and_then(|category| category.upload_remote.clone())
                .or_else(|| settings.upload_remote.clone())
        })
        .flatten()
        .map(|remote| remote.trim().to_owned())
        .filter(|remote| !remote.is_empty());
    // Same precedence again: category override, else the global setting.
    let delete_par2 = category
        .and_then(|category| category.delete_par2)
        .unwrap_or(settings.delete_par2);
    // Same precedence once more, with one difference: an empty category list is not "no
    // override" but "none here", which is how a category switches a globally enabled step off.
    // Steps whose plugin is not installed are dropped while planning rather than queued as
    // rows nothing would ever pick up.
    let plugin_steps: Vec<String> = category
        .and_then(|category| category.plugin_steps.clone())
        .unwrap_or_else(|| settings.plugin_steps.clone())
        .into_iter()
        .filter(|plugin_id| inner.plugin_step_installed(plugin_id))
        .collect();
    let plan = pipeline::plan(&PlanInput {
        level,
        kind: package.kind,
        files: &files,
        sets: &sets,
        sfv: &sfv_indexes,
        script: script.as_deref(),
        cleanup_enabled: rules.is_active(),
        upload: upload.as_deref(),
        delete_par2,
        plugin_steps: &plugin_steps,
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
        inner
            .database
            .set_package_state(package_id, PackageState::Completed, None, None, None)
            .await?;
        forget_import_history(inner, &package, &settings).await;
        return Ok(());
    }

    let _hold = inner.hold.acquire();
    inner
        .database
        .enqueue_postprocess_steps(
            owner.clone(),
            plan.iter()
                .map(|step| (step.kind, step.source.clone(), step.position))
                .collect(),
        )
        .await?;
    inner
        .database
        .set_package_state(package_id, PackageState::Postprocessing, None, None, None)
        .await?;
    let steps = inner.database.list_postprocess_steps(&owner).await?;

    // Loaded before the checks, not inside the unpack: the RAR integrity test asks the
    // archive itself whether it is intact, and an encrypted archive needs the same passwords
    // the unpack would try.
    let password_list = load_password_file(&settings.passwords_file.as_deref().map_or_else(
        || inner.config.default_passwords_file.clone(),
        PathBuf::from,
    ));
    let package_password = inner.database.package_password(package_id).await?;
    let candidates = password_candidates(package_password.as_deref(), &password_list);
    let rar_choice = settings::rar_tool(&settings, inner.config.rar_timeout)?;
    let rar_tool = rar_choice.tool;

    let mut par2_ok = true;
    let mut par2_answered = false;
    if level.repairs() && package.kind == rd_core::DownloadKind::Usenet {
        let outcome = par2_job::run(inner, &owner, &steps, &files, &directory).await?;
        par2_ok = outcome.ok;
        par2_answered = outcome.answered;
        // The return path (RD-107-04). A repair that is short of blocks is not a verdict
        // while recovery volumes are still sitting in the package unfetched: the missing
        // ones are re-queued, the package goes back to downloading, and the completion
        // listener requests this very pipeline again once they have arrived. Only a package
        // with nothing left to fetch falls through to the failure below.
        if !outcome.shortfalls.is_empty() && trigger != ExtractionTrigger::Force {
            let refill = par2_refill::run(inner, &owner, &downloads, &outcome.shortfalls).await?;
            if refill != par2_refill::Refill::Exhausted {
                par2_refill::stand_down(inner, package_id, refill).await?;
                return Ok(());
            }
        }
    }
    // No longer behind `par2_ok`: SFV is the substitute check, so hanging it off PAR2 meant
    // it never ran in exactly the case it was for (RD-104-04).
    let mut sfv_ok = true;
    if level.repairs() && !sfv_indexes.is_empty() {
        sfv_ok = sfv_job::run(inner, &owner, &steps, &sfv_indexes, &directory).await?;
    }
    // And after those two, SABnzbd's `try_rar_check`: only when neither of them answered.
    let mut rar_test_ok = true;
    if level.repairs() && !rar_test_job::rar_sets(&sets).is_empty() {
        if par2_answered {
            rar_test_job::skip(inner, &owner, &steps, &sets, "PAR2 verified this package").await?;
        } else if !sfv_indexes.is_empty() {
            rar_test_job::skip(
                inner,
                &owner,
                &steps,
                &sets,
                "an SFV index verified this package",
            )
            .await?;
        } else {
            rar_test_ok =
                rar_test_job::run(inner, &owner, &steps, &sets, rar_tool.as_ref(), &candidates)
                    .await?;
        }
    }
    let verified = par2_ok && sfv_ok && rar_test_ok;
    // SABnzbd's `safe_postproc`: the *only* place a verification failure decides what else
    // runs. With it off — or for one run, when somebody asked for it anyway — intact
    // archives beside a broken recovery set are unpacked instead of being locked away.
    let safe_postproc = category
        .and_then(|category| category.safe_postproc)
        .unwrap_or(settings.safe_postproc)
        && trigger != ExtractionTrigger::Force;
    let verification_gate = verified || !safe_postproc;

    let mut unpack_ok = true;
    if level.unpacks() && verification_gate && !sets.is_empty() {
        let context = unpack_job::UnpackContext {
            owner: &owner,
            directory: &directory,
            downloads: &downloads,
            candidates: &candidates,
            limits: settings::archive_limits(&settings),
            rar_tool: rar_tool.clone(),
            rar_conflict: rar_choice.conflict.clone(),
            delete_volumes: level.deletes(),
            trigger,
        };
        unpack_ok = unpack_job::run(inner, &context, &steps, &sets).await?;
        // A torrent payload may still be seeding; deleting the inner intermediates that
        // recursion produces would corrupt it, so recursion stays off for torrents.
        let recursive = category
            .and_then(|category| category.recursive_unpack)
            .unwrap_or(settings.recursive_unpack)
            && package.kind != rd_core::DownloadKind::Torrent;
        if recursive && unpack_ok {
            unpack_ok = unpack_nested(inner, &context, &steps, &sets, &rules).await?;
        }
    }
    // Only once everything that could still need the recovery data has succeeded.
    if delete_par2 && level.deletes() && par2_ok && sfv_ok && unpack_ok {
        par2_job::delete_sets(inner, &owner, &steps, &files).await?;
    }
    if level.unpacks() && verification_gate && unpack_ok && rules.is_active() {
        cleanup_job::run(inner, &owner, &directory, &rules).await?;
    }
    // A livestream recording's segments are joined here rather than by the recorder: a
    // six-hour remux has to survive a restart, and a persisted step is what makes that a
    // resumed job instead of a lost one (RD-080-09).
    remux_recording(inner, &owner, &downloads, &directory, &settings).await?;
    // After cleanup, so a plugin sees the package as it will finally be, and before the user
    // script, which stays the last word. Only for a package that got that far: running a
    // checksum step over a half-unpacked package would report a mismatch that says nothing.
    let mut plugin_steps_ok = true;
    if !plugin_steps.is_empty() && verification_gate && unpack_ok {
        let names = package_file_names(&directory).await;
        plugin_steps_ok =
            plugin_step::run(inner, &owner, &plugin_steps, &directory, &names).await?;
    }
    if let Some(name) = &script {
        // SABnzbd only defines 0-3; a failed SFV check is a verification failure like PAR2.
        let status = if !verified {
            3
        } else if !unpack_ok {
            2
        } else if download_failed {
            1
        } else {
            0
        };
        let scripts_dir = inner.scripts_directory(&settings).await?;
        let context = script_job::ScriptContext {
            package_id: owner.clone(),
            package_name: package.name.clone(),
            final_dir: directory.clone(),
            category: category.map(|category| category.name.clone()),
            kind: serde_json::to_string(&package.kind)
                .unwrap_or_default()
                .trim_matches('"')
                .to_owned(),
            status,
        };
        let timeout = std::time::Duration::from_secs(u64::from(settings.script_timeout_seconds));
        let _ = script_job::run(inner, &owner, &scripts_dir, name, &context, timeout).await?;
    }
    let mut upload_ok = true;
    if let Some(remote) = &upload {
        // Moving a seeding torrent's payload would break the seed, so torrents copy.
        let mode = if package.kind == rd_core::DownloadKind::Torrent {
            rclone_job::UploadMode::Copy
        } else {
            rclone_job::UploadMode::from_setting(&settings.upload_mode)
        };
        // `plugin:<id>/<destination>` goes to an installed destination, anything else to
        // rclone. The two paths differ in one way that matters: the plugin one asks the
        // destination to confirm what it holds before anything local is deleted, which
        // `rclone move` cannot offer.
        if let Some(plugin) = storage_upload::parse_plugin_remote(remote) {
            let names = package_file_names(&directory).await;
            upload_ok = storage_upload::run(
                inner,
                &owner,
                plugin,
                &package.name,
                &directory,
                &names,
                mode,
            )
            .await?;
        } else {
            let context = rclone_job::UploadContext {
                remote,
                mode,
                package_name: &package.name,
                directory: &directory,
                executable: settings.rclone_executable.as_deref(),
                vendor_directory: settings.vendor_directory.as_deref(),
            };
            upload_ok = rclone_job::run(inner, &owner, &context).await?;
        }
    }
    let final_state = if verification_gate && unpack_ok && plugin_steps_ok && upload_ok {
        PackageState::Completed
    } else {
        PackageState::Failed
    };
    // Written before the state so the refresh triggered by `package.state` sees it.
    if level.unpacks() && verification_gate && !sets.is_empty() {
        let outcome = if unpack_ok {
            rd_core::ExtractionResult::Success
        } else {
            rd_core::ExtractionResult::Failed
        };
        inner
            .database
            .set_package_extraction(package_id, Some(outcome))
            .await?;
    }
    inner
        .database
        .set_package_state(package_id, final_state, None, None, None)
        .await?;
    if final_state == PackageState::Completed {
        forget_import_history(inner, &package, &settings).await;
    }
    Ok(())
}

/// Passes of nested extraction after the initial one; caps zip-bomb style chains.
const MAX_RECURSIVE_UNPACK_DEPTH: usize = 3;

/// Extracts archives found inside just-extracted archives, re-scanning the package folder
/// (a real filesystem walk — inner archives have no download rows) after every pass.
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
        delete_volumes: true,
        trigger: outer.trigger,
    };
    for _ in 0..MAX_RECURSIVE_UNPACK_DEPTH {
        let mut files = Vec::new();
        collect_files(outer.directory, &mut files)?;
        files.retain(|path| {
            !std::fs::metadata(path).is_ok_and(|meta| rules.is_sample(path, meta.len()))
        });
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

/// Joins a finished recording's segments, when its policy asked for a container.
///
/// Silent for every other kind of package: a download with no recording history has no
/// segments to join, so this costs one field check.
async fn remux_recording(
    inner: &Inner,
    owner: &str,
    downloads: &[rd_core::DownloadFile],
    directory: &std::path::Path,
    settings: &PostprocessSettings,
) -> Result<()> {
    let Some((file, state)) = downloads
        .iter()
        .find_map(|file| file.recording.as_ref().map(|state| (file, state)))
    else {
        return Ok(());
    };
    let target = inner
        .database
        .list_stream_channels()
        .await
        .ok()
        .and_then(|channels| {
            channels
                .into_iter()
                .find(|channel| channel.url == file.source.as_str())
                .map(|channel| channel.recording.remux)
        })
        .unwrap_or_default();
    if target.extension().is_none() {
        return Ok(());
    }
    let Some((ffmpeg, _ffmpeg_lease)) = crate::settings::ffmpeg_tool(settings) else {
        tracing::warn!("a recording asked to be remuxed but ffmpeg was not found");
        return Ok(());
    };
    let stem = std::path::Path::new(&file.file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("recording")
        // The first segment is `<stem>.part001`, so the container drops that suffix.
        .rsplit_once(".part")
        .map_or_else(|| "recording".to_owned(), |(base, _)| base.to_owned());
    let steps = inner
        .database
        .list_postprocess_steps(owner)
        .await
        .unwrap_or_default();
    let context = crate::remux_job::RemuxContext {
        owner,
        directory,
        segments: crate::remux_job::segments_of(state, directory),
        target,
        stem,
        ffmpeg,
    };
    crate::remux_job::run(inner, &steps, &context).await?;
    Ok(())
}

/// The files of a package directory, by name, as a plugin step may see them.
///
/// Built here rather than reusing the planner's list: that one holds absolute paths, and what
/// a step is offered are names relative to its package — it never learns where the package is.
/// Directories are skipped: `source` reads files.
async fn package_file_names(directory: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(directory).await else {
        return names;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.is_ok_and(|kind| kind.is_file())
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    names
}
