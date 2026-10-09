//! Where a hot folder's finds go: the LinkGrabber, the queue or the NZB import.

use super::*;

pub(super) struct DatabaseSink {
    pub(super) database: rd_db::Database,
    pub(super) scheduler: rd_scheduler::SchedulerHandle,
    pub(super) torrent: rd_torrent::TorrentService,
    pub(super) secrets: rd_secrets::SecretStore,
    pub(super) link_check: crate::link_check_service::LinkCheckService,
    pub(super) media_settings: rd_media::SharedMediaSettings,
    pub(super) gallery_settings: rd_gallery::SharedGallerySettings,
}

#[async_trait]
impl IntakeSink for DatabaseSink {
    async fn submit(&self, intake: HotFolderIntake) -> Result<()> {
        let computed = rd_authn::sha256_hex(&intake.content);
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
            // A refusal keeps its code; a store or filesystem failure keeps its cause, which
            // an HTTP answer would have reduced to `internal.error` (audit 1.9.1, RA-API-01).
            let destination = crate::destination::intake_target(
                &self.database,
                &self.scheduler,
                import.category_id,
            )
            .await
            .map_err(|failure| match failure {
                crate::destination::IntakeTargetError::Refused(refusal) => {
                    anyhow::anyhow!("{} ({})", refusal.message(), refusal.code())
                }
                crate::destination::IntakeTargetError::Failed(error) => error,
            })?;
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

    /// Records a drop moved to `processed` without a second import (audit 1.9.1, RA-IN-02).
    ///
    /// While the first import's row exists, that row is the record: it holds the same content
    /// under the same digest. Only when it is gone — removed between the import and the move —
    /// did the drop reach nobody, and it gets the same failed row a refused NZB gets.
    async fn record_duplicate(&self, duplicate: rd_hotfolder::DuplicateIntake) {
        let imports = match self.database.list_nzb_imports().await {
            Ok(imports) => imports,
            Err(error) => {
                tracing::warn!(%error, "hotfolder could not look up the NZB history for a duplicate");
                return;
            }
        };
        let in_history = imports
            .iter()
            .any(|import| import.sha256 == duplicate.sha256);
        let Some(record) = nzb_duplicate_record(&duplicate, in_history) else {
            return;
        };
        if let Err(error) = self.database.record_nzb_import_failure(record).await {
            tracing::warn!(
                %error,
                path = %duplicate.source_path.display(),
                "hotfolder could not record the duplicate NZB drop"
            );
        }
    }
}

impl DatabaseSink {
    /// Imports a container into the LinkGrabber.
    ///
    /// Unlike an NZB or a torrent this always stops at the LinkGrabber, even in `Enqueue`
    /// mode: starting the links means resolving them through plugins, captchas and accounts,
    /// which only the request path can do. The links are checked automatically, so an
    /// `Enqueue` folder still surfaces them ready to start with one click.
    pub(super) async fn submit_container(
        &self,
        intake: HotFolderIntake,
        format: rd_collector::ContainerFormat,
    ) -> Result<()> {
        if format == rd_collector::ContainerFormat::RdLinks {
            return self.submit_links(intake).await;
        }
        let document = if format.needs_service() {
            let settings = crate::settings_store::stored_settings(&self.database)
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
        } else if format == rd_collector::ContainerFormat::CrawlJob {
            rd_collector::read_crawljob(&intake.content)?
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

    /// Imports an `.rdlinks` file into the LinkGrabber (RD-1210-01).
    ///
    /// Only a readable one: a folder has nobody to ask for a passphrase, so a sealed file is
    /// refused and moved to `failed/` like any other drop that cannot be read. The links are
    /// proposals of a document the person dropped themselves, so they may reach the person's own
    /// network and never this machine — `LinkOrigin::Proposed` of an intake by their own hand.
    /// The NZBs it carries land as a dropped NZB does, in the folder's mode (RD-1220-02).
    async fn submit_links(&self, intake: HotFolderIntake) -> Result<()> {
        let document = crate::links_file::read_links(&intake.content, None)
            .await
            .map_err(|error| anyhow::anyhow!(error.message().to_owned()))?;
        let source_label = intake
            .source_path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned);
        let sink = crate::dlc_import::DlcIntake {
            database: &self.database,
            secrets: &self.secrets,
            link_check: &self.link_check,
            media: self.media_settings.read().await.clone(),
            gallery: self.gallery_settings.read().await.clone(),
        };
        crate::dlc_import::import_links(
            &sink,
            document,
            crate::dlc_import::DlcImportOptions {
                source: rd_core::IngressSource::HotFolder,
                source_label,
                fallback_name: None,
                fallback_password: None,
                category_id: intake.category_id,
                priority: None,
            },
            Some(true),
            if intake.mode == rd_core::ImportMode::Enqueue {
                crate::links_nzb::NzbLanding::Enqueue(&self.scheduler)
            } else {
                crate::links_nzb::NzbLanding::Review
            },
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.message().to_owned()))?;
        Ok(())
    }

    pub(super) async fn submit_torrent(&self, intake: HotFolderIntake) -> Result<()> {
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
            crate::torrent_intake::add_torrent_to_collector(
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
        crate::torrent_intake::enqueue_torrent_with(
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
