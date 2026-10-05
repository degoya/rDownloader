use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{HotFolderConfig, HotFolderExecutor, HotFolderId};
use rd_hotfolder::{HotFolderIntake, IntakeSink, PollInterval, WatchOptions};

use crate::{ApiError, dto::SettingsResponse};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

mod sink;

use sink::*;

/// Owns daemon-side watchers and restores them from SQLite on startup.
#[derive(Clone)]
pub struct HotFolderService {
    database: rd_db::Database,
    scheduler: rd_scheduler::SchedulerHandle,
    torrent: rd_torrent::TorrentService,
    /// Vault for credentials a `.dlc` link carries in its URL.
    secrets: rd_secrets::SecretStore,
    /// Probes the links a `.dlc` brings in, exactly as a pasted batch is probed.
    link_check: crate::link_check_service::LinkCheckService,
    media_settings: rd_media::SharedMediaSettings,
    gallery_settings: rd_gallery::SharedGallerySettings,
    cancellation: CancellationToken,
    tasks: Arc<Mutex<HashMap<HotFolderId, tokio::task::JoinHandle<Result<()>>>>>,
    /// The reconciliation interval every watcher follows; a saved setting lands here
    /// (RD-110-31).
    poll_interval: PollInterval,
}

impl HotFolderService {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        database: rd_db::Database,
        scheduler: rd_scheduler::SchedulerHandle,
        torrent: rd_torrent::TorrentService,
        secrets: rd_secrets::SecretStore,
        link_check: crate::link_check_service::LinkCheckService,
        media_settings: rd_media::SharedMediaSettings,
        gallery_settings: rd_gallery::SharedGallerySettings,
    ) -> Self {
        Self {
            database,
            scheduler,
            torrent,
            secrets,
            link_check,
            media_settings,
            gallery_settings,
            cancellation: CancellationToken::new(),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            poll_interval: PollInterval::default(),
        }
    }

    /// Starts all enabled daemon configurations after recovery, at the interval the settings
    /// document holds.
    pub async fn start_existing(&self) -> Result<()> {
        let settings: rd_core::HotFolderSettings =
            self.database.service_settings_or_default().await?;
        self.poll_interval.set(settings.poll_interval());
        for config in self.database.list_hotfolders().await? {
            self.start(config).await?;
        }
        Ok(())
    }

    /// Starts one daemon watcher; capture-agent folders remain assigned to that agent.
    pub async fn start(&self, config: HotFolderConfig) -> Result<()> {
        if !config.enabled || !matches!(config.executor, HotFolderExecutor::Daemon) {
            return Ok(());
        }
        let mut tasks = self.tasks.lock().await;
        if still_running(&mut tasks, config.id).await {
            return Ok(());
        }
        let id = config.id;
        let handle = rd_hotfolder::spawn(
            config,
            Arc::new(DatabaseSink {
                database: self.database.clone(),
                scheduler: self.scheduler.clone(),
                torrent: self.torrent.clone(),
                secrets: self.secrets.clone(),
                link_check: self.link_check.clone(),
                media_settings: self.media_settings.clone(),
                gallery_settings: self.gallery_settings.clone(),
            }),
            self.cancellation.child_token(),
            WatchOptions {
                reconciliation_interval: self.poll_interval.clone(),
                ..WatchOptions::default()
            },
        );
        tasks.insert(id, handle);
        Ok(())
    }

    /// Hands a changed interval to every running watcher; each re-arms its ticker in place.
    pub fn set_poll_interval(&self, interval: Duration) {
        self.poll_interval.set(interval);
    }

    /// The interval the watchers follow right now.
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval.get()
    }

    /// Stops one watcher, e.g. before an update recreates it with a new path or after a
    /// delete. Does nothing when the folder is not watched (disabled or capture-agent side).
    pub async fn stop(&self, id: HotFolderId) {
        let Some(task) = self.tasks.lock().await.remove(&id) else {
            return;
        };
        task.abort();
        match task.await {
            Ok(Ok(())) | Err(_) => {}
            Ok(Err(error)) => tracing::warn!(%error, "hotfolder stopped with error"),
        }
    }

    /// Stops all reconciliation and native watcher tasks.
    pub async fn shutdown(&self) {
        self.cancellation.cancel();
        let tasks = std::mem::take(&mut *self.tasks.lock().await);
        for (_, task) in tasks {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::warn!(%error, "hotfolder stopped with error"),
                Err(error) => tracing::warn!(%error, "hotfolder task failed"),
            }
        }
    }
}

/// Whether the watcher for `id` is still running. A finished one - a configuration that can
/// never work ends its task - is taken out and its result logged, so saving the folder again
/// starts a new watcher instead of being refused by a dead handle.
async fn still_running(
    tasks: &mut HashMap<HotFolderId, tokio::task::JoinHandle<Result<()>>>,
    id: HotFolderId,
) -> bool {
    match tasks.get(&id) {
        None => return false,
        Some(task) if !task.is_finished() => return true,
        Some(_) => {}
    }
    if let Some(task) = tasks.remove(&id) {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::error!(%error, "hotfolder watcher had stopped"),
            Err(error) => tracing::error!(%error, "hotfolder watcher task failed"),
        }
    }
    false
}

/// The poll interval the settings document asks for, checked at save time.
pub fn validate_hotfolder_settings(settings: &SettingsResponse) -> Result<(), ApiError> {
    let range = rd_core::HOTFOLDER_POLL_SECONDS_RANGE;
    if !range.contains(&settings.hotfolder_poll_seconds) {
        return Err(ApiError::bad_request(
            "settings.hotfolder_poll_invalid",
            format!(
                "The hotfolder poll interval must be between {} and {} seconds",
                range.start(),
                range.end()
            ),
        )
        .with_param("min", range.start())
        .with_param("max", range.end())
        .with_param("seconds", settings.hotfolder_poll_seconds));
    }
    Ok(())
}

/// The interval as the watchers need it.
pub fn poll_interval_of(settings: &SettingsResponse) -> Duration {
    Duration::from_secs(u64::from(settings.hotfolder_poll_seconds))
}

/// What a refused drop leaves behind, or `None` when it is not an NZB at all.
fn nzb_failure_record(failure: &rd_hotfolder::FailedIntake) -> Option<rd_db::FailedNzbImport> {
    if !has_extension(&failure.source_path, "nzb") {
        return None;
    }
    // The same name the successful path stores, so the row reads like any other import; the
    // password marker goes with it, because nothing here can use a password.
    let name = failure
        .source_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(rd_files::strip_password_marker)
        .map(|(name, _)| rd_files::sanitize_file_name(&name))
        .unwrap_or_else(|| "hotfolder.nzb".to_owned());
    Some(rd_db::FailedNzbImport {
        name,
        sha256: failure.sha256.clone(),
        source_path: Some(path_string(&failure.source_path)),
        error: failure.reason.clone(),
    })
}

/// The failed row for a duplicate NZB drop whose first import is no longer in the history;
/// `None` while it is, or for any other kind of file.
fn nzb_duplicate_record(
    duplicate: &rd_hotfolder::DuplicateIntake,
    in_history: bool,
) -> Option<rd_db::FailedNzbImport> {
    if in_history {
        return None;
    }
    nzb_failure_record(&rd_hotfolder::FailedIntake {
        source_path: duplicate.source_path.clone(),
        sha256: duplicate.sha256.clone(),
        failed_path: duplicate.processed_path.clone(),
        reason: format!(
            "The same file was taken in moments before and its import was removed since; moved to {} without a second import",
            duplicate.processed_path.display()
        ),
    })
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
#[path = "hotfolder_service_tests.rs"]
mod tests;
