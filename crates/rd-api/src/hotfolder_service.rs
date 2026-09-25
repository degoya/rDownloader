use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{HotFolderConfig, HotFolderExecutor, HotFolderId};
use rd_hotfolder::{HotFolderIntake, IntakeSink, PollInterval, WatchOptions};

use crate::{ApiError, dto::SettingsResponse};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

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
        if tasks.contains_key(&config.id) {
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

struct DatabaseSink {
    database: rd_db::Database,
    scheduler: rd_scheduler::SchedulerHandle,
    torrent: rd_torrent::TorrentService,
    secrets: rd_secrets::SecretStore,
    link_check: crate::link_check_service::LinkCheckService,
    media_settings: rd_media::SharedMediaSettings,
    gallery_settings: rd_gallery::SharedGallerySettings,
}

#[async_trait]
impl IntakeSink for DatabaseSink {
    async fn submit(&self, intake: HotFolderIntake) -> Result<()> {
        let computed = hex::encode(Sha256::digest(&intake.content));
        anyhow::ensure!(
            computed == intake.sha256,
            "hotfolder hash changed before intake"
        );
        if has_extension(&intake.source_path, "torrent") {
            return self.submit_torrent(intake).await;
        }
        if let Some(format) = intake
            .source_path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(rd_collector::ContainerFormat::from_file_name)
        {
            return self.submit_container(intake, format).await;
        }
        let parsed = rd_collector::parse_nzb(&intake.content)?;
        let (name, marker_password) = intake
            .source_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(rd_files::strip_password_marker)
            .map(|(name, password)| (rd_files::sanitize_file_name(&name), password))
            .unwrap_or_else(|| ("hotfolder.nzb".to_owned(), None));
        let password = marker_password.or_else(|| parsed.password.clone());
        let mode = intake.mode;
        let import = self
            .database
            .add_nzb_import(rd_db::NewNzbImport {
                name,
                sha256: intake.sha256,
                category_id: intake.category_id,
                // A folder without a category of its own leaves the decision to the routing
                // rules, which can now target the drop by `source = hotfolder`.
                source: rd_core::IngressSource::HotFolder,
                priority: None,
                import_mode: intake.mode,
                source_path: Some(path_string(&intake.source_path)),
                password,
                // A watched folder picking one up is news.
                announce_arrival: true,
                files: parsed
                    .files
                    .into_iter()
                    .map(|file| rd_db::NewNzbFile {
                        subject: file.subject,
                        poster: file.poster,
                        groups: file.groups,
                        segments: file
                            .segments
                            .into_iter()
                            .map(|segment| rd_db::NewNzbSegment {
                                number: segment.number,
                                bytes: segment.bytes,
                                message_id: segment.message_id,
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .await?;
        if mode == rd_core::ImportMode::Enqueue && !import.duplicate {
            // The category the import actually got, not the folder's: when a routing rule or
            // the default category decided, the files belong where that category points.
            let destination =
                crate::destination::resolve_destination(&self.database, import.category_id)
                    .await?
                    .unwrap_or_else(|| self.scheduler.downloads_directory().to_path_buf());
            if let Some(shortfall) =
                crate::storage_capacity::intake_block(&self.scheduler.capacity(), &destination)
                    .await
            {
                anyhow::bail!(
                    "storage root is below its free-space threshold ({} bytes free, {} required)",
                    shortfall.free_bytes,
                    shortfall.minimum_free_bytes
                );
            }
            self.database
                // A watched folder configured to enqueue means "start it"; pausing is a
                // LinkGrabber decision, so this path never starts paused.
                .enqueue_nzb_import(
                    import.id,
                    destination,
                    rd_core::DownloadPriority::Normal,
                    false,
                )
                .await?;
        }
        Ok(())
    }

    /// Leaves the trace RD-108-20 is about: a drop nobody made by hand reached nobody at all.
    ///
    /// Only an NZB gets a row, because `nzb_imports` is the NZB table. A torrent or a container
    /// that fails keeps the log line and the file under `failed/` it always had; giving those
    /// two a home of their own is a separate piece of work, not a side effect of this one.
    async fn record_failure(&self, failure: rd_hotfolder::FailedIntake) {
        let Some(record) = nzb_failure_record(&failure) else {
            return;
        };
        if let Err(error) = self.database.record_nzb_import_failure(record).await {
            tracing::warn!(
                %error,
                path = %failure.source_path.display(),
                "hotfolder could not record the failed NZB import"
            );
        }
    }
}

/// The poll interval the settings document asks for, checked at save time.
pub(crate) fn validate_hotfolder_settings(settings: &SettingsResponse) -> Result<(), ApiError> {
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
pub(crate) fn poll_interval_of(settings: &SettingsResponse) -> Duration {
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

impl DatabaseSink {
    /// Imports a container into the LinkGrabber.
    ///
    /// Unlike an NZB or a torrent this always stops at the LinkGrabber, even in `Enqueue`
    /// mode: starting the links means resolving them through plugins, captchas and accounts,
    /// which only the request path can do. The links are checked automatically, so an
    /// `Enqueue` folder still surfaces them ready to start with one click.
    async fn submit_container(
        &self,
        intake: HotFolderIntake,
        format: rd_collector::ContainerFormat,
    ) -> Result<()> {
        let document = if format.needs_service() {
            let settings = crate::handlers::stored_settings(&self.database)
                .await
                .map_err(|error| anyhow::anyhow!(error.message().to_owned()))?;
            crate::dlc_import::decrypt_container(
                &settings,
                &intake.content,
                format.service_source(),
            )
            .await
            .map_err(|error| anyhow::anyhow!(error.message().to_owned()))?
        } else if format == rd_collector::ContainerFormat::Rsdf {
            rd_collector::decode_rsdf(&intake.content)?
        } else {
            rd_collector::parse_link_list(&intake.content)
        };
        let source_label = intake
            .source_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned);
        let (stem, password) =
            rd_files::strip_password_marker(source_label.as_deref().unwrap_or("hotfolder.dlc"));
        let fallback_name = {
            let base = stem
                .rsplit_once('.')
                .map_or(stem.as_str(), |(base, _)| base);
            let stem = rd_files::sanitize_file_name(base.trim());
            (!stem.is_empty()).then_some(stem)
        };
        if intake.mode == rd_core::ImportMode::Enqueue {
            tracing::info!(
                path = %intake.source_path.display(),
                format = format.as_str(),
                "a container always lands in the LinkGrabber; the folder's enqueue mode does not apply"
            );
        }
        let sink = crate::dlc_import::DlcIntake {
            database: &self.database,
            secrets: &self.secrets,
            link_check: &self.link_check,
            media: self.media_settings.read().await.clone(),
            gallery: self.gallery_settings.read().await.clone(),
        };
        crate::dlc_import::import_document(
            &sink,
            document,
            crate::dlc_import::DlcImportOptions {
                source: rd_core::IngressSource::HotFolder,
                source_label,
                fallback_name,
                fallback_password: password,
                category_id: intake.category_id,
                priority: None,
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.message().to_owned()))?;
        Ok(())
    }

    async fn submit_torrent(&self, intake: HotFolderIntake) -> Result<()> {
        anyhow::ensure!(
            intake.content.len() <= rd_torrent::MAX_TORRENT_BYTES,
            "torrent file exceeds the 16 MiB limit"
        );
        let source_label = intake
            .source_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned);
        if intake.mode == rd_core::ImportMode::Review {
            crate::torrent_handlers::add_torrent_to_collector(
                &self.database,
                &self.torrent,
                &intake.content,
                rd_core::IngressSource::HotFolder,
                source_label,
                None,
                intake.category_id,
                None,
            )
            .await?;
            return Ok(());
        }
        let (parsed, stored) = self.torrent.store_torrent_file(&intake.content).await?;
        let source = url::Url::from_file_path(&stored)
            .map_err(|()| anyhow::anyhow!("stored torrent path is not absolute"))?;
        crate::torrent_handlers::enqueue_torrent_with(
            &self.database,
            &self.scheduler,
            source,
            parsed.name,
            Some(parsed.total_bytes),
            intake.category_id,
            rd_core::DownloadPriority::Normal,
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.message().to_owned()))?;
        Ok(())
    }
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
mod tests {
    use std::{path::PathBuf, time::Duration};

    use super::{nzb_failure_record, poll_interval_of, validate_hotfolder_settings};
    use crate::dto::SettingsResponse;

    /// RD-110-31: the bounds, and the code a value outside them is refused with.
    #[test]
    fn the_poll_interval_is_bounded() {
        let mut settings = SettingsResponse::default();
        assert!(validate_hotfolder_settings(&settings).is_ok());
        assert_eq!(poll_interval_of(&settings), Duration::from_secs(30));
        for seconds in [5, 3600] {
            settings.hotfolder_poll_seconds = seconds;
            assert!(validate_hotfolder_settings(&settings).is_ok(), "{seconds}");
        }
        for seconds in [0, 4, 3601] {
            settings.hotfolder_poll_seconds = seconds;
            assert_eq!(
                validate_hotfolder_settings(&settings)
                    .expect_err("outside the range")
                    .code(),
                "settings.hotfolder_poll_invalid",
                "{seconds}"
            );
        }
    }

    fn failure(name: &str) -> rd_hotfolder::FailedIntake {
        rd_hotfolder::FailedIntake {
            source_path: PathBuf::from("/watch").join(name),
            sha256: "a".repeat(64),
            failed_path: PathBuf::from("/watch/failed").join(name),
            reason: "NZB could not be parsed".to_owned(),
        }
    }

    #[test]
    fn a_refused_nzb_becomes_a_record_under_the_name_a_successful_one_would_have() {
        let record = nzb_failure_record(&failure("Release{{secret}}.nzb")).expect("record");
        assert_eq!(record.name, "Release.nzb");
        assert_eq!(record.sha256, "a".repeat(64));
        assert_eq!(
            record.source_path.as_deref(),
            Some("/watch/Release{{secret}}.nzb")
        );
        assert_eq!(record.error, "NZB could not be parsed");
    }

    /// `nzb_imports` is the NZB table; the other three container kinds have no row to leave.
    #[test]
    fn a_refused_container_leaves_no_nzb_record() {
        for name in ["batch.torrent", "links.dlc", "links.ccf", "links.rsdf"] {
            assert!(
                nzb_failure_record(&failure(name)).is_none(),
                "{name} produced an NZB record"
            );
        }
        assert!(nzb_failure_record(&failure("release.NZB")).is_some());
    }
}
