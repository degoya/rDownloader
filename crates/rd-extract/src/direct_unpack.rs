//! Direct unpack (RD-1100-07): a Usenet package's multi-volume RAR set is unpacked while the
//! package is still downloading, volume by volume as each one arrives — SABnzbd's
//! `directunpacker`.
//!
//! The completion listener starts one unpack per set once the set's first volume has completed
//! ([`on_volume_completed`]); the tool then waits for every further volume until its download
//! has completed, and gives up on one that ends in any other way — missing articles leave a file
//! `Verifying`, a pause leaves it `Paused`. What it writes stays in a staging directory of its
//! own in the package folder. The pipeline takes the finished results when it starts
//! ([`DirectUnpacks::settle`]) and only moves one into the package once the verification passed
//! without a repair: PAR2 wins, and a set it had to repair is unpacked again the normal way.
//!
//! Nothing of this is persisted. A restart forgets which staging directory belongs to which set,
//! so the next pipeline removes every one it holds no result for ([`sweep`]) and unpacks the
//! set the normal way.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use rd_core::{ByteCount, DownloadFile, DownloadKind, PackageId};
use rd_postprocess::{
    ArchiveSet, DIRECT_STAGING_PREFIX, DirectRequest, ExternalRarTool, ExtractionReport,
    RarToolKind,
};
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{ExtractionTrigger, Inner, package_settings::PackageSettings, settings};

#[path = "direct_unpack_volumes.rs"]
mod volumes;

use volumes::{arriving, await_volume};

/// A finished direct unpack, waiting for the pipeline to adopt or discard it.
#[derive(Clone, Debug)]
pub(crate) struct Staged {
    /// The set's first volume, as the pipeline names it (`package folder` + file name).
    pub(crate) first_volume: PathBuf,
    pub(crate) staging: PathBuf,
    pub(crate) report: ExtractionReport,
    /// The file names of the volumes the tool read, in order.
    pub(crate) volumes: Vec<String>,
}

impl Staged {
    /// Whether this is `set`'s unpack: the same first volume, and nothing read that is not one
    /// of the set's volumes.
    pub(crate) fn is_for(&self, set: &ArchiveSet) -> bool {
        self.first_volume == set.first()
            && self.volumes.iter().all(|name| {
                set.volumes
                    .iter()
                    .any(|volume| volume.file_name() == Some(std::ffi::OsStr::new(name)))
            })
    }
}

/// One package's direct unpacks: each set's first volume and the task unpacking it.
type PackageUnpacks = Vec<(PathBuf, JoinHandle<Option<Staged>>)>;

/// The direct unpacks of every package, running or finished, until the pipeline takes them.
#[derive(Default)]
pub(crate) struct DirectUnpacks(Mutex<HashMap<PackageId, PackageUnpacks>>);

impl DirectUnpacks {
    /// Takes `package_id`'s unpacks out of the registry and returns the finished ones.
    ///
    /// `wait`: an automatic run waits for an unpack still working — every download has settled
    /// by then, so it either finishes the last volumes or gives up straight away. A manual run
    /// does not wait for downloads that may still be running; it stops them and unpacks the
    /// normal way. What a stopped or failed unpack leaves is for [`sweep`].
    pub(crate) async fn settle(&self, package_id: PackageId, wait: bool) -> Vec<Staged> {
        let entries = self.0.lock().await.remove(&package_id).unwrap_or_default();
        let mut finished = Vec::new();
        for (_, handle) in entries {
            if !wait {
                handle.abort();
            }
            if let Ok(Some(staged)) = handle.await {
                if wait {
                    finished.push(staged);
                } else {
                    discard(std::slice::from_ref(&staged)).await;
                }
            }
        }
        finished
    }

    /// Registers `first_volume`'s unpack, unless one for it already is.
    async fn claim(
        &self,
        package_id: PackageId,
        first_volume: &Path,
        start: impl FnOnce() -> JoinHandle<Option<Staged>>,
    ) {
        let mut packages = self.0.lock().await;
        let entries = packages.entry(package_id).or_default();
        if entries.iter().any(|(first, _)| first == first_volume) {
            return;
        }
        entries.push((first_volume.to_owned(), start()));
    }
}

/// Removes every direct unpack staging directory in `directory` that is not one of `keep`:
/// what a stop, a restart or a manual run left behind.
pub(crate) async fn sweep(directory: &Path, keep: &[Staged]) {
    let Ok(mut entries) = tokio::fs::read_dir(rd_files::long_path(directory)).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let stale = name.to_string_lossy().starts_with(DIRECT_STAGING_PREFIX)
            && !keep
                .iter()
                .any(|staged| staged.staging.file_name() == Some(name.as_os_str()));
        if stale
            && entry.file_type().await.is_ok_and(|kind| kind.is_dir())
            && let Err(error) = tokio::fs::remove_dir_all(entry.path()).await
        {
            tracing::warn!(
                path = %entry.path().display(),
                %error,
                "a direct unpack's staging directory could not be removed"
            );
        }
    }
}

/// Removes the staging directories of `staged`; one already adopted is gone and skipped.
pub(crate) async fn discard(staged: &[Staged]) {
    for staged in staged {
        if let Err(error) = tokio::fs::remove_dir_all(&staged.staging).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %staged.staging.display(),
                %error,
                "a direct unpack's staging directory could not be removed"
            );
        }
    }
}

/// Moves a direct unpack into `destination`: `Some` with its report once it is there, `None`
/// when the move failed and the set has to be unpacked the normal way. That way overwrites
/// whatever the failed move already put there, as a rerun after a crash does.
pub(crate) async fn adopt(staged: &Staged, destination: &Path) -> Result<Option<ExtractionReport>> {
    // A stop here leaves the whole tree in staging and nothing at the destination; the next
    // start removes the staging and unpacks the set the normal way (recovery matrix).
    rd_core::failpoint!(
        "postprocess.before_direct_unpack_adopted",
        || anyhow::anyhow!("crash point")
    );
    let staging = staged.staging.clone();
    let target = destination.to_owned();
    let moved =
        tokio::task::spawn_blocking(move || rd_postprocess::adopt_direct(&staging, &target)).await;
    let error = match moved {
        Ok(Ok(())) => return Ok(Some(staged.report)),
        Ok(Err(error)) => error,
        Err(error) => anyhow::Error::new(error),
    };
    tracing::warn!(%error, "a direct unpack could not be moved into place; unpacking again");
    Ok(None)
}

/// Starts a direct unpack when `download_id` completed the first volume of a multi-volume RAR
/// set in a package that asked for it. Logs instead of failing: the set is unpacked after the
/// download either way.
pub(crate) async fn on_volume_completed(inner: &Arc<Inner>, download_id: rd_core::DownloadId) {
    if let Err(error) = start_if_wanted(inner, download_id).await {
        tracing::debug!(%error, "direct unpack not started");
    }
}

async fn start_if_wanted(inner: &Arc<Inner>, download_id: rd_core::DownloadId) -> Result<()> {
    let database = &inner.database;
    let Some(download) = database.get_download(download_id).await? else {
        return Ok(());
    };
    // The cheap question first: almost every completed file is not the start of a RAR set.
    let Some(volume) = rd_files::parse_archive_volume(&download.file_name) else {
        return Ok(());
    };
    if volume.kind != rd_files::ArchiveKind::Rar || !volume.is_first {
        return Ok(());
    }
    let Some(package) = database.get_package(download.package_id).await? else {
        return Ok(());
    };
    if package.kind != DownloadKind::Usenet
        || matches!(
            package.state,
            rd_core::PackageState::Postprocessing
                | rd_core::PackageState::Completed
                | rd_core::PackageState::Failed
        )
    {
        return Ok(());
    }
    let downloads = database.downloads_for_package(package.id).await?;
    // Nothing is left to wait for: the pipeline is about to unpack the set anyway.
    if !downloads.iter().any(arriving) {
        return Ok(());
    }
    // A bare `Film.rar` is a set only with old-style `Film.r00` volumes beside it.
    let multi_volume = volume.numbered
        || downloads.iter().any(|file| {
            rd_files::parse_archive_volume(&file.file_name).is_some_and(|other| {
                other.kind == rd_files::ArchiveKind::Rar
                    && other.index > 0
                    && other.base.eq_ignore_ascii_case(&volume.base)
            })
        });
    if !multi_volume {
        return Ok(());
    }
    let postprocess = settings::load_postprocess_settings(database).await?;
    let categories = database.list_categories().await?;
    let category = package
        .category_id
        .and_then(|id| categories.iter().find(|category| category.id == id));
    let chosen = PackageSettings::resolve(
        inner,
        &package,
        category,
        &postprocess,
        ExtractionTrigger::Auto,
    );
    if !chosen.direct_unpack || !chosen.level.unpacks() {
        return Ok(());
    }
    // Quiet hours defer the heavy steps; the set is unpacked after the download instead.
    if inner.config.quiet_hold.is_held() {
        return Ok(());
    }
    let directory = PathBuf::from(&package.destination);
    let first_volume = directory.join(&download.file_name);
    if !first_volume.is_file() {
        return Ok(());
    }
    let choice =
        settings::refuse_outdated(settings::rar_tool(&postprocess, inner.config.rar_timeout)?)
            .await;
    let Some(tool) = choice.tool.filter(|tool| tool.kind == RarToolKind::Unrar) else {
        tracing::info!(
            package_id = %package.id,
            "direct unpack needs unrar; the set is unpacked after the download"
        );
        return Ok(());
    };
    if !room_for_it(inner, &directory, &downloads).await {
        return Ok(());
    }
    let password = database.package_password(package.id).await?;
    let limits = settings::archive_limits(&postprocess);
    let package_id = package.id;
    let task_inner = Arc::clone(inner);
    let first = first_volume.clone();
    inner
        .direct
        .claim(package_id, &first_volume, move || {
            tokio::spawn(run(
                task_inner, package_id, directory, first, tool, limits, password,
            ))
        })
        .await;
    Ok(())
}

/// Whether the package's filesystem holds the unpacked set beside what is still arriving.
///
/// The unpacked set is taken to be as large as the whole package, PAR2 files and all — an
/// overestimate, which only ever means unpacking after the download — plus the bytes still to
/// come and the free space every root keeps.
async fn room_for_it(inner: &Inner, directory: &Path, downloads: &[DownloadFile]) -> bool {
    let unpacked: u64 = downloads
        .iter()
        .filter_map(|file| file.total_bytes.map(ByteCount::get))
        .sum();
    let arriving_bytes: u64 = downloads
        .iter()
        .filter(|file| arriving(file))
        .map(|file| {
            file.total_bytes
                .map_or(0, ByteCount::get)
                .saturating_sub(file.committed_bytes.get())
        })
        .sum();
    let storage: rd_core::StorageSettings =
        inner.database.service_settings().await.unwrap_or_default();
    let required = unpacked
        .saturating_add(arriving_bytes)
        .saturating_add(storage.storage_minimum_free_bytes.get());
    match rd_files::available_space(directory).await {
        Ok(free) if free >= required => true,
        Ok(free) => {
            tracing::info!(
                free,
                required,
                "not enough free space to unpack while downloading; the set is unpacked after the download"
            );
            false
        }
        Err(error) => {
            tracing::debug!(%error, "free space unknown; the set is unpacked after the download");
            false
        }
    }
}

/// One set's unpack, from its first volume to its last; `None` when it gave up.
async fn run(
    inner: Arc<Inner>,
    package_id: PackageId,
    directory: PathBuf,
    first_volume: PathBuf,
    tool: ExternalRarTool,
    limits: rd_postprocess::ArchiveLimits,
    password: Option<String>,
) -> Option<Staged> {
    tracing::info!(%package_id, volume = %first_volume.display(), "direct unpack started");
    // In a block of its own, so the extraction's borrows end before `first_volume` moves on.
    let outcome = {
        let (asks, mut questions) = tokio::sync::mpsc::channel(1);
        let extraction = rd_postprocess::extract_rar_direct(DirectRequest {
            tool: &tool,
            first_volume: &first_volume,
            parent: &directory,
            limits,
            password: password.as_deref(),
            asks,
        });
        tokio::pin!(extraction);
        loop {
            tokio::select! {
                outcome = &mut extraction => break outcome,
                Some(ask) = questions.recv() => {
                    let go_on = await_volume(&inner, package_id, &directory, &ask.volume).await;
                    let _ = ask.reply.send(go_on);
                }
            }
        }
    };
    match outcome {
        Ok(staging) => {
            tracing::info!(
                %package_id,
                volumes = staging.volumes.len(),
                files = staging.report.files,
                "direct unpack finished; the pipeline moves it into place after the verification"
            );
            Some(Staged {
                first_volume,
                staging: staging.staging,
                report: staging.report,
                volumes: staging.volumes,
            })
        }
        Err(error) => {
            tracing::info!(
                %package_id,
                code = error.code(),
                %error,
                "direct unpack given up; the set is unpacked after the download"
            );
            None
        }
    }
}
