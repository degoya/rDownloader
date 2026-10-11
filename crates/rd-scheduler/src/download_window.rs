//! The download window (RD-1240-30): a bandwidth profile that pauses downloads while it is in
//! force, and a package's or category's own weekly window.
//!
//! A held package's waiting files stay where they are — the dispatch pass leaves them out, as it
//! does a package whose "not before" lies ahead — so a link added meanwhile waits with them, and
//! nothing has to be undone when the hold ends. Its running transfers that can resume are paused
//! the way "pause all" pauses them: recorded first, then paused one by one, and once the hold
//! ends the recorded files still paused are resumed and no others. The record is its own, not the
//! queue pause's, so a pause somebody set is never lifted by a window's end, and a window's end
//! never resumes a file somebody paused by hand.
//!
//! What cannot resume runs to its end: a live recording, a media, gallery or plugin transfer,
//! and an HTTP transfer of unknown size or from a host that ignores ranges. A recording is not
//! held back at all — a live stream does not wait for a window. Seeding and post-processing are
//! no download and are not touched; uploads keep their own limit.

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{
    CategoryId, DownloadFile, DownloadId, DownloadKind, DownloadPackage, DownloadState,
    DownloadWindow, PackageId,
};
use rd_limits::{DownloadHold, package_hold};
use serde::{Deserialize, Serialize};

use crate::{SchedulerHandle, StopReason};

/// The files a download window paused; JSON `{"files": []}` when none.
const SCHEDULE_HOLD_KEY: &str = "queue.schedule_hold";

/// The files the download window paused, so its end resumes exactly those.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct ScheduleHold {
    files: Vec<DownloadId>,
}

/// Which packages wait at one instant, and why.
#[derive(Debug, Default)]
pub(crate) struct DownloadGate {
    // Why, for the log and the REST layer; the dispatch pass only asks whether.
    held: HashMap<PackageId, DownloadHold>,
}

impl DownloadGate {
    /// Whether `file` may not start or keep downloading now. A recording is never held: a live
    /// stream does not wait for a window.
    pub(crate) fn holds(&self, file: &DownloadFile) -> bool {
        file.kind != DownloadKind::Record && self.held.contains_key(&file.package_id)
    }
}

impl SchedulerHandle {
    /// The gate at `now` over `packages`: the profile in force and each package's window — its
    /// own, otherwise its category's.
    pub(crate) async fn download_gate(
        &self,
        packages: &[DownloadPackage],
        now: DateTime<Utc>,
    ) -> Result<DownloadGate> {
        let (schedule_pauses, timezone) = self.config.bandwidth.download_pause_policy().await;
        // The categories are read only when a package may inherit a window from one.
        let categories: HashMap<CategoryId, DownloadWindow> = if packages
            .iter()
            .any(|package| package.download_window.is_none() && package.category_id.is_some())
        {
            self.database
                .list_categories()
                .await?
                .into_iter()
                .filter_map(|category| category.download_window.map(|window| (category.id, window)))
                .collect()
        } else {
            HashMap::new()
        };
        let held = packages
            .iter()
            .filter_map(|package| {
                let category = package.category_id.and_then(|id| categories.get(&id));
                let window = DownloadWindow::effective(package.download_window.as_ref(), category);
                package_hold(window, schedule_pauses, timezone, now).map(|hold| (package.id, hold))
            })
            .collect();
        Ok(DownloadGate { held })
    }

    /// What holds `package` back right now, if anything (RD-1240-30); for the REST layer.
    pub async fn package_download_hold(
        &self,
        package: &DownloadPackage,
    ) -> Result<Option<DownloadHold>> {
        let gate = self
            .download_gate(std::slice::from_ref(package), Utc::now())
            .await?;
        Ok(gate.held.get(&package.id).copied())
    }

    /// One supervision step: pauses the running transfers of held packages that can resume, and
    /// resumes the ones it paused once their package may download again.
    pub(crate) async fn supervise_download_windows(&self) -> Result<()> {
        let stored = self.load_schedule_hold().await?;
        let running: Vec<DownloadId> = self.active.lock().await.tokens.keys().copied().collect();
        if stored.files.is_empty() && running.is_empty() {
            return Ok(());
        }
        let packages = self.database.list_packages().await?;
        let gate = self.download_gate(&packages, Utc::now()).await?;
        let mut kept = Vec::new();
        let mut released = Vec::new();
        for id in &stored.files {
            let Some(file) = self.database.get_download(*id).await? else {
                continue;
            };
            let stopping = self.active.lock().await.reasons.get(id) == Some(&StopReason::Paused);
            match file.state {
                DownloadState::Paused if !gate.holds(&file) => released.push(*id),
                DownloadState::Paused => kept.push(*id),
                // Asked to pause and not there yet; the next step finds it paused.
                state if state.is_working() && stopping => kept.push(*id),
                // Resumed, cancelled or finished by somebody else: no longer this hold's.
                _ => {}
            }
        }
        let mut stopping = Vec::new();
        for id in running {
            let Some(file) = self.database.get_download(id).await? else {
                continue;
            };
            if !gate.holds(&file) || !self.can_resume(&file) || kept.contains(&id) {
                continue;
            }
            // A stop already asked for — a pause, a cancel, a block — is its asker's.
            if self.active.lock().await.reasons.contains_key(&id) {
                continue;
            }
            tracing::debug!(
                download = %id,
                hold = ?gate.held.get(&file.package_id),
                "a running transfer pauses for its download window"
            );
            stopping.push(id);
        }
        // Recorded before any file is touched, so a stop in between leaves nothing paused that
        // the hold's end would not find; forgotten after the resumes, for the same reason.
        if !stopping.is_empty() {
            let mut recorded = stored.files.clone();
            recorded.extend(stopping.iter().copied());
            self.store_schedule_hold(&ScheduleHold { files: recorded })
                .await?;
        }
        for id in &stopping {
            if let Err(error) = self.pause(*id).await {
                tracing::debug!(%error, download = %id, "a file could not be paused by its download window");
            }
        }
        for id in &released {
            if let Err(error) = self.resume(*id).await {
                tracing::warn!(%error, download = %id, "a file could not be resumed after its download window");
            }
        }
        kept.extend(stopping);
        if kept != stored.files {
            self.store_schedule_hold(&ScheduleHold { files: kept })
                .await?;
        }
        if !released.is_empty() {
            tracing::info!(
                resumed = released.len(),
                "the download window let paused files go on"
            );
        }
        Ok(())
    }

    /// Whether a running transfer can be paused now and pick up where it stopped later.
    fn can_resume(&self, file: &DownloadFile) -> bool {
        if !matches!(
            file.state,
            DownloadState::Resolving | DownloadState::Downloading
        ) {
            return false;
        }
        // Nothing has been fetched yet while the source is still being asked.
        if file.state == DownloadState::Resolving {
            return file.kind != DownloadKind::Record;
        }
        match file.kind {
            DownloadKind::Http => {
                file.total_bytes.is_some() && !self.host_limits().ignores_ranges(&file.source)
            }
            DownloadKind::Usenet
            | DownloadKind::Torrent
            | DownloadKind::Ftp
            | DownloadKind::Sftp
            | DownloadKind::ObjectStorage => true,
            DownloadKind::Media
            | DownloadKind::Gallery
            | DownloadKind::Record
            | DownloadKind::Plugin => false,
        }
    }

    async fn load_schedule_hold(&self) -> Result<ScheduleHold> {
        let Some(stored) = self.database.get_setting(SCHEDULE_HOLD_KEY).await? else {
            return Ok(ScheduleHold::default());
        };
        Ok(serde_json::from_value(stored).unwrap_or_else(|error| {
            tracing::warn!(%error, "the stored download window record was unreadable and is ignored");
            ScheduleHold::default()
        }))
    }

    async fn store_schedule_hold(&self, hold: &ScheduleHold) -> Result<()> {
        self.database
            .set_setting(SCHEDULE_HOLD_KEY.to_owned(), serde_json::to_value(hold)?)
            .await
    }
}

#[cfg(test)]
#[path = "download_window_tests.rs"]
mod tests;
