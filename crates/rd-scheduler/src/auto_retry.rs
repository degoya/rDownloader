//! The automatic retry of failed downloads (RD-191-12).
//!
//! The retries of `retry.rs` end once a download's attempts or limit waits are spent, and the
//! download is `failed` from then on: a hoster that was down for a day, or a daily limit that
//! outlasted the waits, ended the job for good. Switched on, this takes such a download up
//! again an interval after it failed, a bounded number of rounds, each with a fresh retry
//! budget.
//!
//! Two writes per round, each a single statement of its own: the due time is set on the
//! failed row (`next_retry_at`, which the queue shows as "next attempt at"), and once it has
//! passed the row goes back to `queued` with the round counted. A stop between the two leaves a
//! failed row with a due time, which the next pass takes up exactly as if nothing had happened.

use std::sync::atomic::Ordering;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rd_core::{DownloadFile, FailureKind};

use crate::SchedulerHandle;

/// Default hours between a failure and its automatic retry.
pub const DEFAULT_AUTO_RETRY_INTERVAL_HOURS: u32 = 6;
/// The shortest interval: an hour, the wait of a rate limit without a stated reset.
pub const MIN_AUTO_RETRY_INTERVAL_HOURS: u32 = 1;
/// The longest interval: a day, the period of a daily limit.
pub const MAX_AUTO_RETRY_INTERVAL_HOURS: u32 = 24;
/// Default rounds per download.
pub const DEFAULT_AUTO_RETRY_MAX_ROUNDS: u32 = 3;
/// The most rounds that can be configured; `0` means no limit.
pub const MAX_AUTO_RETRY_ROUNDS: u32 = 100;

/// The automatic retry's settings as one value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AutoRetry {
    pub(crate) enabled: bool,
    pub(crate) interval_hours: u32,
    /// `0` is no limit.
    pub(crate) max_rounds: u32,
}

impl AutoRetry {
    fn rounds_left(self, spent: u32) -> bool {
        self.max_rounds == 0 || spent < self.max_rounds
    }

    fn interval(self) -> chrono::Duration {
        chrono::Duration::hours(i64::from(self.interval_hours))
    }
}

/// Whether a failed download's failure may pass if it is tried again later.
///
/// What a later attempt can change: a limit, an IP block, a server or network that was down,
/// a file the hoster reported offline or unavailable for legal reasons (451). Never what it
/// cannot: a link that is gone, a refused or invalid account, a missing login, a captcha
/// nobody solved, a kind nobody handles, wrong bytes — all of those fail the same way again,
/// or cost a paid solve each time. A mirror given up for another one is not taken up either:
/// the group's turn has moved on, and two members of one group would contend for it.
pub(crate) fn may_pass_later(file: &DownloadFile) -> bool {
    let Some(failure) = file.last_error.as_ref() else {
        return false;
    };
    if failure.code.as_deref() == Some(crate::mirrors::HANDOVER_CODE) {
        return false;
    }
    matches!(
        failure.category,
        FailureKind::Transient { .. }
            | FailureKind::Offline
            | FailureKind::RateLimited { .. }
            | FailureKind::IpBlocked { .. }
    )
}

impl SchedulerHandle {
    pub(crate) fn auto_retry(&self) -> AutoRetry {
        AutoRetry {
            enabled: self.auto_retry_failed.load(Ordering::Acquire),
            interval_hours: self.auto_retry_interval_hours.load(Ordering::Acquire),
            max_rounds: self.auto_retry_max_rounds.load(Ordering::Acquire),
        }
    }

    /// One pass of the automatic retry; called by the supervise loop once a minute.
    pub(crate) async fn supervise_auto_retry(&self) -> Result<()> {
        self.auto_retry_pass(Utc::now()).await.map(|_| ())
    }

    /// Sets, moves or clears the due time of every failed download and puts the ones that are
    /// due back into the queue. Returns how many it put back.
    ///
    /// Switched off it does nothing, except once after it was on (and once after a start): the
    /// due times it had set are cleared then, so the queue does not announce a retry that will
    /// not come.
    pub(crate) async fn auto_retry_pass(&self, now: DateTime<Utc>) -> Result<usize> {
        let settings = self.auto_retry();
        let was_enabled = self
            .auto_retry_was_enabled
            .swap(settings.enabled, Ordering::AcqRel);
        if !settings.enabled && !was_enabled {
            return Ok(0);
        }
        let mut requeued = 0;
        for candidate in self.database.auto_retry_candidates().await? {
            let file = &candidate.download;
            let wanted = settings.enabled
                && may_pass_later(file)
                && settings.rounds_left(candidate.rounds)
                && !self.group_moved_on(file).await?;
            if !wanted {
                if file.next_retry_at.is_some() {
                    self.database.schedule_auto_retry(file.id, None).await?;
                }
                continue;
            }
            match file.next_retry_at {
                Some(due) if due <= now => {
                    // Between the decision and the write (RD-191-12): a stop here leaves the
                    // row failed with its due time and its round uncounted, which the next
                    // pass takes up as it would have.
                    rd_core::failpoint!(
                        "scheduler.before_auto_retry_requeued",
                        || anyhow::anyhow!("crash point: scheduler.before_auto_retry_requeued")
                    );
                    if self.database.auto_retry_download(file.id).await?.is_some() {
                        requeued += 1;
                        tracing::info!(
                            download_id = %file.id,
                            round = candidate.rounds.saturating_add(1),
                            "a failed download was put back into the queue by the automatic retry"
                        );
                    }
                }
                // Already set, and no later than the interval allows: a shortened interval
                // brings a due time nearer, a longer one leaves it where it is.
                Some(due) if due <= now + settings.interval() => {}
                _ => {
                    self.database
                        .schedule_auto_retry(file.id, Some(now + settings.interval()))
                        .await?;
                }
            }
        }
        Ok(requeued)
    }

    /// Whether another member of the file's mirror group holds the turn or has finished: a
    /// retry of this one would download a second copy.
    async fn group_moved_on(&self, file: &DownloadFile) -> Result<bool> {
        if file.mirror_group.is_none() {
            return Ok(false);
        }
        let downloads = self.database.downloads_for_package(file.package_id).await?;
        Ok(crate::mirrors::siblings(file, &downloads)
            .iter()
            .any(|sibling| crate::mirrors::holds_the_group_open(sibling.state)))
    }
}
