//! Channel monitor: probes enabled livestream channels on an interval and starts a
//! recording package as soon as a channel goes live.

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use rd_core::{DownloadState, StreamChannelId};
use rd_db::Database;
use rd_scheduler::{FileSpec, PackageSpec, SchedulerHandle};
use tokio_util::sync::CancellationToken;
use url::Url;

/// After this many consecutive probe failures a channel is probed only every 4th cycle.
const ERROR_BACKOFF_THRESHOLD: u32 = 3;

struct Inner {
    database: Database,
    scheduler: SchedulerHandle,
    settings: rd_stream::SharedStreamSettings,
    shutdown: CancellationToken,
}

/// Cloneable handle of the background monitor loop.
#[derive(Clone)]
pub struct StreamMonitorService {
    inner: Arc<Inner>,
}

impl StreamMonitorService {
    #[must_use]
    pub fn start(
        database: Database,
        scheduler: SchedulerHandle,
        settings: rd_stream::SharedStreamSettings,
    ) -> Self {
        let service = Self {
            inner: Arc::new(Inner {
                database,
                scheduler,
                settings,
                shutdown: CancellationToken::new(),
            }),
        };
        tokio::spawn(service.clone().run());
        service
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// Plans and expires now, outside the tick.
    ///
    /// Called when a schedule is created or edited, so its occurrences exist immediately
    /// instead of after a poll interval — nobody should have to wait a minute to see when
    /// their recording will happen.
    pub async fn plan_now(&self) {
        if let Err(error) = self.plan_and_expire().await {
            tracing::warn!(%error, "stream schedule planning failed");
        }
    }

    async fn run(self) {
        let mut errors: HashMap<StreamChannelId, u32> = HashMap::new();
        let mut cycle: u64 = 0;
        // Once before the first sleep: a window that closed while the service was down is
        // still a missed recording, and it should say so at startup rather than a minute in.
        self.plan_now().await;
        loop {
            let interval = {
                let settings = self.inner.settings.read().await;
                Duration::from_secs(u64::from(settings.record_poll_interval_seconds.max(60)))
            };
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                () = tokio::time::sleep(interval) => {}
            }
            cycle += 1;
            // Planning first: a window that opens this cycle must already have its run row,
            // or the recording it starts would have nothing to attach itself to.
            if let Err(error) = self.plan_and_expire().await {
                tracing::warn!(%error, "stream schedule planning failed");
            }
            if let Err(error) = self.poll_channels(&mut errors, cycle).await {
                tracing::warn!(%error, "stream channel poll failed");
            }
        }
    }

    /// Plans upcoming occurrences and closes the ones whose window has passed
    /// (RD-080-08).
    ///
    /// Both halves are idempotent on purpose. Planning inserts against a UNIQUE index, so
    /// running it every cycle and again after a restart adds nothing the second time;
    /// expiring only touches runs that are still open, so a window that passed while the
    /// service was down becomes a visible *missed* row rather than staying "planned"
    /// forever.
    async fn plan_and_expire(&self) -> Result<()> {
        let now = chrono::Utc::now();
        let horizon = now + chrono::Duration::days(rd_core::PLANNING_HORIZON_DAYS);
        for schedule in self.inner.database.enabled_stream_schedules().await? {
            let occurrences = match rd_stream::occurrences(&schedule, now, horizon) {
                Ok(occurrences) => occurrences,
                Err(error) => {
                    // A schedule that cannot be resolved is skipped, not fatal: one broken
                    // row must not stop every other schedule from being planned.
                    tracing::warn!(
                        schedule = %schedule.name,
                        code = error.code(),
                        "schedule could not be resolved"
                    );
                    continue;
                }
            };
            let planned: Vec<rd_db::PlannedOccurrence> = occurrences
                .into_iter()
                .map(|occurrence| rd_db::PlannedOccurrence {
                    starts_at: occurrence.starts_at,
                    ends_at: occurrence.ends_at,
                })
                .collect();
            if planned.is_empty() {
                continue;
            }
            let created = self
                .inner
                .database
                .plan_stream_runs(schedule.id, schedule.channel_id, planned)
                .await?;
            if created > 0 {
                tracing::info!(schedule = %schedule.name, created, "planned recordings");
            }
        }
        // A run whose window closed without the channel going live is a missed recording,
        // and saying so is the point: silently dropping it hides the failure.
        let expired = self.inner.database.expire_stream_runs(now).await?;
        if expired > 0 {
            tracing::info!(expired, "scheduled recordings were missed");
        }
        Ok(())
    }

    /// The open run whose watch window covers `now` for this channel, if any.
    async fn active_run(
        &self,
        channel_id: StreamChannelId,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<(rd_core::StreamScheduledRun, rd_core::StreamSchedule)> {
        let schedules = self.inner.database.enabled_stream_schedules().await.ok()?;
        let runs = self.inner.database.open_stream_runs().await.ok()?;
        runs.into_iter().find_map(|run| {
            if run.channel_id != channel_id {
                return None;
            }
            let schedule = schedules
                .iter()
                .find(|schedule| schedule.id == run.schedule_id)?;
            let occurrence = rd_stream::Occurrence {
                starts_at: run.starts_at,
                ends_at: run.ends_at,
            };
            rd_stream::is_watching(schedule, &occurrence, now).then(|| (run, schedule.clone()))
        })
    }

    /// Whether this channel has any schedule at all.
    ///
    /// A channel with schedules is watched *only* inside them; one without keeps the old
    /// behaviour of being polled round the clock, so adding the feature changes nothing for
    /// anybody who does not use it.
    async fn is_scheduled(&self, channel_id: StreamChannelId) -> bool {
        self.inner
            .database
            .enabled_stream_schedules()
            .await
            .map(|schedules| {
                schedules
                    .iter()
                    .any(|schedule| schedule.channel_id == channel_id)
            })
            .unwrap_or(false)
    }

    async fn poll_channels(
        &self,
        errors: &mut HashMap<StreamChannelId, u32>,
        cycle: u64,
    ) -> Result<()> {
        let settings = self.inner.settings.read().await.clone();
        let Some(streamlink) = rd_stream::locate_streamlink(&settings) else {
            // No tool, no probes; the tool status card already surfaces this.
            return Ok(());
        };
        let channels = self.inner.database.list_stream_channels().await?;
        let downloads = self.inner.database.list_downloads().await?;
        for channel in channels.into_iter().filter(|channel| channel.enabled) {
            let failures = errors.get(&channel.id).copied().unwrap_or(0);
            if failures >= ERROR_BACKOFF_THRESHOLD && !cycle.is_multiple_of(4) {
                continue;
            }
            // One recording per channel at a time: skip while a Record row for this URL
            // is still active (a finished one allows the next recording immediately).
            let recording = downloads.iter().any(|file| {
                file.kind == rd_core::DownloadKind::Record
                    && file.source.as_str() == channel.url
                    && !matches!(
                        file.state,
                        DownloadState::Completed
                            | DownloadState::Failed
                            | DownloadState::Cancelled
                            | DownloadState::Blocked
                    )
            });
            if recording {
                continue;
            }
            // A scheduled channel is watched only inside its window. Polling it round the
            // clock would defeat the point of scheduling it — and would record the wrong
            // broadcast if the channel goes live for something else (RD-080-08).
            let now = chrono::Utc::now();
            let active = self.active_run(channel.id, now).await;
            if active.is_none() && self.is_scheduled(channel.id).await {
                continue;
            }
            match rd_stream::probe_stream(&streamlink.path, &channel.url).await {
                Ok(probe) if probe.live => {
                    errors.remove(&channel.id);
                    let _ = self
                        .inner
                        .database
                        .touch_stream_channel(channel.id, Some(chrono::Utc::now()), None)
                        .await;
                    match start_recording(
                        &self.inner.database,
                        &self.inner.scheduler,
                        &channel.url,
                        &channel.name,
                        channel.quality.as_deref(),
                        channel.category_id,
                        &settings,
                    )
                    .await
                    {
                        Err(error) => {
                            tracing::warn!(channel = %channel.name, %error, "recording could not be started");
                            if let Some((run, _)) = &active {
                                let _ = self
                                    .inner
                                    .database
                                    .set_stream_run_state(
                                        run.id,
                                        rd_core::ScheduledRunState::Failed,
                                        None,
                                        None,
                                        Some(rd_core::redact_text(&error.to_string())),
                                    )
                                    .await;
                            }
                        }
                        Ok(package) => {
                            tracing::info!(channel = %channel.name, "channel is live, recording started");
                            if let Some((run, schedule)) = &active {
                                // Replay is recorded as what actually happened, not as what
                                // was asked for: the UI must never claim a capability the
                                // provider did not offer.
                                let replay_used =
                                    schedule.replay_from_start && probe.replay_available;
                                let download_id =
                                    self.inner.database.list_downloads().await.ok().and_then(
                                        |files| {
                                            files
                                                .into_iter()
                                                .find(|file| file.package_id == package.id)
                                                .map(|file| file.id)
                                        },
                                    );
                                let _ = self
                                    .inner
                                    .database
                                    .set_stream_run_state(
                                        run.id,
                                        rd_core::ScheduledRunState::Recording,
                                        download_id,
                                        Some(replay_used),
                                        None,
                                    )
                                    .await;
                            }
                        }
                    }
                }
                Ok(_) => {
                    errors.remove(&channel.id);
                    let _ = self
                        .inner
                        .database
                        .touch_stream_channel(channel.id, None, None)
                        .await;
                }
                Err(error) => {
                    *errors.entry(channel.id).or_default() += 1;
                    let _ = self
                        .inner
                        .database
                        .touch_stream_channel(channel.id, None, Some(error.to_string()))
                        .await;
                }
            }
        }
        Ok(())
    }
}

/// Enqueues one recording package (used by the monitor and the "record now" endpoint).
pub async fn start_recording(
    database: &Database,
    scheduler: &SchedulerHandle,
    url: &str,
    name: &str,
    quality: Option<&str>,
    category_id: Option<rd_core::CategoryId>,
    settings: &rd_core::StreamSettings,
) -> Result<rd_core::DownloadPackage> {
    let source = Url::parse(url).context("channel URL is not a valid URL")?;
    let destination = crate::destination::resolve_destination(database, category_id)
        .await?
        .unwrap_or_else(|| scheduler.downloads_directory().to_path_buf());
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let base = rd_files::sanitize_file_name(&format!("{name}-{stamp}"));
    let quality = quality
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&settings.record_default_quality)
        .to_owned();
    // The media selection carries the stream quality to the runner; `format` is the
    // streamlink stream name, not an extractor format expression. There are therefore no
    // format criteria to resolve — `effective_criteria` returns `None` for a quality name,
    // and the stream runner reads `format` verbatim.
    let selection = rd_core::MediaSelection {
        page_url: source.clone(),
        variant_id: quality.clone(),
        format: quality,
        kind: rd_core::MediaKind::Video,
        ext: "ts".to_owned(),
        title: name.to_owned(),
        contract_version: rd_core::MEDIA_CONTRACT_VERSION,
        criteria: None,
        resolved: None,
    };
    let (package, _) = scheduler
        .enqueue_package(
            PackageSpec {
                name: base.clone(),
                destination,
                category_id,
                priority: rd_core::DownloadPriority::default(),
                password: None,
                start_paused: false,
                postprocess_level: None,
                script: None,
                // Nothing looked at this: it is started from what the person chose.
                enrichment: Vec::new(),
            },
            vec![FileSpec {
                source,
                file_name: format!("{base}.ts"),
                size: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                kind: rd_core::DownloadKind::Record,
                replay: None,
                media: Some(selection),
                remote_credential_id: None,
                // A recording is one file with one source; there is nothing to mirror it with.
                mirror_group: None,
                skipped: false,
                // Nothing looked at this: it is started from what the person chose.
                enrichment: Vec::new(),
                secret_fragment: None,
            }],
        )
        .await?;
    Ok(package)
}
