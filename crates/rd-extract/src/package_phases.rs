//! The phases of one package's pipeline, in the order `package_job::run_package` runs them:
//! verify, unpack, cleanup, remux, malware scan, plugin steps, user script, upload.
//!
//! Each phase does its work; whether it runs at all, and what its answer means for the rest,
//! is decided in `run_package`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::{PostprocessSettings, PostprocessStep};
use rd_postprocess::{group_archive_sets, load_password_file, password_candidates};

use crate::{
    ExtractionTrigger, Inner, cleanup_job, malware_scan, object_upload,
    package_job::{Run, package_file_names},
    par2_job, par2_refill, plugin_step, rar_test_job, rclone_job, script_job, settings, sfv_job,
    steps::StepEnd,
    storage_upload, unpack_job,
};

/// The passwords to try and the RAR tool, shared by the integrity test and the unpack.
pub(crate) struct ArchiveTools {
    pub(crate) candidates: Vec<Option<String>>,
    pub(crate) rar_choice: settings::RarToolChoice,
}

/// Loaded before the checks, not inside the unpack: the RAR integrity test asks the
/// archive itself whether it is intact, and an encrypted archive needs the same passwords
/// the unpack would try.
pub(crate) async fn archive_tools(run: &Run<'_>) -> Result<ArchiveTools> {
    let inner = run.inner;
    let password_list = load_password_file(&run.settings.passwords_file.as_deref().map_or_else(
        || inner.config.default_passwords_file.clone(),
        PathBuf::from,
    ));
    let package_password = inner.database.package_password(run.package_id).await?;
    let candidates = password_candidates(package_password.as_deref(), &password_list);
    let rar_choice =
        settings::refuse_outdated(settings::rar_tool(&run.settings, inner.config.rar_timeout)?)
            .await;
    Ok(ArchiveTools {
        candidates,
        rar_choice,
    })
}

/// What the three checks found.
pub(crate) struct Verification {
    pub(crate) par2_ok: bool,
    pub(crate) sfv_ok: bool,
    pub(crate) rar_test_ok: bool,
    /// PAR2 found damaged or missing files, whether or not it could repair them.
    pub(crate) repaired: bool,
}

/// PAR2 (Usenet), SFV and the RAR integrity test.
///
/// `None` when a repair short of blocks sent the package back to downloading the recovery
/// volumes it still has unfetched.
pub(crate) async fn verify(
    run: &Run<'_>,
    steps: &[PostprocessStep],
    tools: &ArchiveTools,
) -> Result<Option<Verification>> {
    let inner = run.inner;
    let owner = &run.owner;
    let level = run.chosen.level;
    let sets = &run.sets;
    let sfv_indexes = &run.sfv_indexes;
    let rar_choice = &tools.rar_choice;
    let mut par2_ok = true;
    let mut par2_answered = false;
    let mut repaired = false;
    if level.repairs() && run.package.kind == rd_core::DownloadKind::Usenet {
        let outcome = par2_job::run(inner, owner, steps, &run.files, &run.directory).await?;
        par2_ok = outcome.ok;
        par2_answered = outcome.answered;
        repaired = outcome.repaired;
        // The return path (RD-107-04). A repair that is short of blocks is not a verdict
        // while recovery volumes are still sitting in the package unfetched: the missing
        // ones are re-queued, the package goes back to downloading, and the completion
        // listener requests this very pipeline again once they have arrived. Only a package
        // with nothing left to fetch falls through to the failure below.
        if !outcome.shortfalls.is_empty() && run.trigger != ExtractionTrigger::Force {
            let refill =
                par2_refill::run(inner, owner, &run.downloads, &outcome.shortfalls).await?;
            if refill != par2_refill::Refill::Exhausted {
                par2_refill::stand_down(inner, run.package_id, refill).await?;
                return Ok(None);
            }
        }
    }
    // No longer behind `par2_ok`: SFV is the substitute check, so hanging it off PAR2 meant
    // it never ran in exactly the case it was for (RD-104-04).
    let mut sfv_ok = true;
    if level.repairs() && !sfv_indexes.is_empty() {
        sfv_ok = sfv_job::run(inner, owner, steps, sfv_indexes, &run.directory).await?;
    }
    // And after those two, SABnzbd's `try_rar_check`: only when neither of them answered.
    let mut rar_test_ok = true;
    if level.repairs() && !rar_test_job::rar_sets(sets).is_empty() {
        if par2_answered {
            rar_test_job::skip(
                inner,
                owner,
                steps,
                sets,
                crate::steps::codes::RAR_TEST_SKIPPED_PAR2,
                "PAR2 verified this package",
            )
            .await?;
        } else if !sfv_indexes.is_empty() {
            rar_test_job::skip(
                inner,
                owner,
                steps,
                sets,
                crate::steps::codes::RAR_TEST_SKIPPED_SFV,
                "an SFV index verified this package",
            )
            .await?;
        } else if direct_covers(&run.direct, sets) {
            // `unrar` checked every file's checksum while it unpacked the set during the
            // download; testing the same volumes again would only cost a pass (RD-1100-07).
            rar_test_job::skip(
                inner,
                owner,
                steps,
                sets,
                crate::steps::codes::RAR_TEST_SKIPPED_DIRECT,
                "the set was unpacked while it downloaded",
            )
            .await?;
        } else if rar_choice.outdated.is_some() {
            // Not started at all; the unpack step names the version and the floor.
            rar_test_job::skip(
                inner,
                owner,
                steps,
                sets,
                crate::steps::codes::RAR_TEST_SKIPPED_OUTDATED,
                "the RAR tool is below its security floor",
            )
            .await?;
        } else {
            rar_test_ok = rar_test_job::run(
                inner,
                owner,
                steps,
                sets,
                rar_choice.tool.as_ref(),
                &tools.candidates,
            )
            .await?;
        }
    }
    Ok(Some(Verification {
        par2_ok,
        sfv_ok,
        rar_test_ok,
        repaired,
    }))
}

/// Whether every RAR set of the package was unpacked while it downloaded.
fn direct_covers(
    direct: &[crate::direct_unpack::Staged],
    sets: &[rd_postprocess::ArchiveSet],
) -> bool {
    let rar = rar_test_job::rar_sets(sets);
    !rar.is_empty()
        && rar
            .iter()
            .all(|set| direct.iter().any(|staged| staged.is_for(set)))
}

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

/// Removes what the cleanup rules name and returns what it removed.
pub(crate) async fn clean_up(run: &Run<'_>) -> Result<Vec<String>> {
    // A folder per archive is a level of its own; the content below it is cleaned as deep
    // as it would be in the package folder (RD-190-06).
    let depth = cleanup_job::depth(run.chosen.unpack_to_subfolder && !run.sets.is_empty());
    cleanup_job::run(
        run.inner,
        &run.owner,
        &run.directory,
        &run.chosen.rules,
        depth,
    )
    .await
}

/// Joins a finished recording's segments, when its policy asked for a container.
///
/// Silent for every other kind of package: a download with no recording history has no
/// segments to join, so this costs one field check.
pub(crate) async fn remux_recording(
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

/// The malware scan; `false` on a finding, which also skips every step after it.
pub(crate) async fn scan(run: &Run<'_>) -> Result<bool> {
    let inner = run.inner;
    let scan_clean = malware_scan::run(
        inner,
        &run.owner,
        &run.package,
        &run.directory,
        &run.settings,
    )
    .await?;
    if !scan_clean {
        malware_scan::skip_after_finding(inner, &run.owner).await?;
    }
    Ok(scan_clean)
}

/// The package's plugin steps, told what was downloaded, what the package held before the
/// pipeline (`before`) and what the cleanup removed (`cleaned`).
pub(crate) async fn plugin_steps(
    run: &Run<'_>,
    before: &[String],
    cleaned: &[String],
) -> Result<StepEnd> {
    let downloaded: Vec<String> = run
        .downloads
        .iter()
        .map(|file| plugin_step::relative_name(Path::new(&file.file_name)))
        .collect();
    // The files and what was removed are listed per step there: a step may rename.
    plugin_step::run(
        run.inner,
        &run.owner,
        &run.chosen.plugin_steps,
        &run.directory,
        before,
        &downloaded,
        cleaned,
    )
    .await
}

/// The status a user script is told.
pub(crate) const fn script_status(verified: bool, unpack_ok: bool, download_failed: bool) -> u8 {
    // SABnzbd only defines 0-3; a failed SFV check is a verification failure like PAR2.
    if !verified {
        3
    } else if !unpack_ok {
        2
    } else if download_failed {
        1
    } else {
        0
    }
}

/// Runs the user script `name`; what it answered is recorded on its step, never a verdict.
pub(crate) async fn script(run: &Run<'_>, name: &str, status: u8) -> Result<()> {
    let inner = run.inner;
    let settings = &run.settings;
    let scripts_dir = inner.scripts_directory(settings).await?;
    let context = script_job::ScriptContext {
        package_id: run.owner.clone(),
        package_name: run.package.name.clone(),
        final_dir: run.directory.clone(),
        category: run.category.clone(),
        kind: serde_json::to_string(&run.package.kind)
            .unwrap_or_default()
            .trim_matches('"')
            .to_owned(),
        status,
    };
    let timeout = std::time::Duration::from_secs(u64::from(settings.script_timeout_seconds));
    let _ = script_job::run(inner, &run.owner, &scripts_dir, name, &context, timeout).await?;
    Ok(())
}

/// Uploads the package to `remote` and says how the upload step ended.
pub(crate) async fn upload(run: &Run<'_>, remote: &str) -> Result<StepEnd> {
    let inner = run.inner;
    let owner = &run.owner;
    let package = &run.package;
    let directory = &run.directory;
    let settings = &run.settings;
    // Moving a seeding torrent's payload would break the seed, so torrents copy.
    let mode = if package.kind == rd_core::DownloadKind::Torrent {
        rclone_job::UploadMode::Copy
    } else {
        rclone_job::UploadMode::from_setting(&settings.upload_mode)
    };
    // `object-storage:<profile>/<bucket>/<prefix>` goes to object storage,
    // `plugin:<id>/<destination>` to an installed destination, anything else to rclone. The two paths differ in one way that matters: the plugin one asks the
    // destination to confirm what it holds before anything local is deleted, which
    // `rclone move` cannot offer.
    if let Some(target) = object_upload::parse_object_remote(remote) {
        let names = package_file_names(directory).await;
        object_upload::run(
            inner,
            owner,
            remote,
            target,
            &package.name,
            directory,
            &names,
            mode,
        )
        .await
    } else if let Some(plugin) = storage_upload::parse_plugin_remote(remote) {
        let names = package_file_names(directory).await;
        storage_upload::run(inner, owner, remote, plugin, directory, &names, mode).await
    } else {
        let context = rclone_job::UploadContext {
            remote,
            mode,
            package_name: &package.name,
            directory,
            executable: settings.rclone_executable.as_deref(),
            vendor_directory: settings.vendor_directory.as_deref(),
            bwlimit: inner
                .upload_limit()
                .binding_limit()
                .map(|limit| limit.bytes_per_second),
        };
        rclone_job::run(inner, owner, &context).await
    }
}
