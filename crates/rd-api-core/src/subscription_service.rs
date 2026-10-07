//! The background poller (RD-080-07).
//!
//! Follows the shape every long-running task in this application uses — a private `Inner`
//! behind an `Arc`, a `CancellationToken`, and a `tokio::select!` loop — with three
//! properties that are specific to polling other people's servers:
//!
//! * **One source's failure is its own.** Each subscription's poll is isolated and its error
//!   is stored on its own row. A dead indexer must not stop a channel from being checked.
//! * **Nothing is queued twice.** The archive decides what is new, and it decides in the
//!   database, so an interrupted poll repeated after a restart changes nothing.
//! * **The first poll is not an import.** Until a subscription is primed, the backlog policy
//!   applies, and it defaults to ignoring everything that already exists.
//!
//! A subscription with a cron expression (RD-130-19) runs at the times it names, in the
//! service's local zone, instead of every interval. The time itself comes from a clock the
//! service is given, so a test can move it rather than wait for six in the morning.
//!
//! An accepted item is archived as pending and becomes queued only once the LinkGrabber has it
//! (RD-190-13). A stop in between, a refused intake or a file whose address could not be had
//! leaves it in the review list, where it can be queued by hand; the archive never claims a
//! download nobody started.

use std::sync::Arc;

use rd_core::{
    BacklogPolicy, Subscription, SubscriptionItemState, SubscriptionMode, SubscriptionSettings,
};
use rd_db::{NewSubscriptionItem, PollResult};
use rd_subscription::SourceAdapter;
use tokio_util::sync::CancellationToken;

use rd_db::Database;

use crate::{link_check_service::LinkCheckService, subscription_hosts::HostGate};

mod adapters;
mod git_fetch;
#[cfg(test)]
mod git_release_tests;
#[cfg(test)]
mod indexer_poll_tests;
mod intake;
mod poll;
mod scheduled;
#[cfg(test)]
mod tests;

pub use adapters::{
    ArchivedItems, HttpFeedFetcher, SandboxScriptRunner, SharedSiteRules, VaultSecretResolver,
};
pub use git_fetch::HttpApiFetcher;
pub use intake::{SubscriptionIntake, hand_urls_to_intake};

/// Where the poller reads the time from: the wall clock in production, a hand-moved one in a
/// test (RD-130-19).
pub(crate) type Clock = Arc<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>;

struct Inner {
    database: Database,
    link_check: LinkCheckService,
    media_settings: rd_media::SharedMediaSettings,
    gallery_settings: rd_gallery::SharedGallerySettings,
    adapters: Vec<Arc<dyn SourceAdapter>>,
    /// One request at a time per indexer, and a gap between them (RD-101-18).
    hosts: HostGate,
    shutdown: CancellationToken,
    clock: Clock,
}

/// Cloneable handle of the poll loop.
#[derive(Clone)]
pub struct SubscriptionService {
    inner: Arc<Inner>,
}

impl SubscriptionService {
    #[must_use]
    pub fn start(
        database: Database,
        link_check: LinkCheckService,
        media_settings: rd_media::SharedMediaSettings,
        gallery_settings: rd_gallery::SharedGallerySettings,
        adapters: Vec<Arc<dyn SourceAdapter>>,
    ) -> Self {
        Self::start_with_clock(
            database,
            link_check,
            media_settings,
            gallery_settings,
            adapters,
            Arc::new(chrono::Utc::now),
        )
    }

    /// The same, reading the time from `clock` (RD-130-19).
    pub(crate) fn start_with_clock(
        database: Database,
        link_check: LinkCheckService,
        media_settings: rd_media::SharedMediaSettings,
        gallery_settings: rd_gallery::SharedGallerySettings,
        adapters: Vec<Arc<dyn SourceAdapter>>,
        clock: Clock,
    ) -> Self {
        let service = Self {
            inner: Arc::new(Inner {
                database,
                link_check,
                media_settings,
                gallery_settings,
                adapters,
                hosts: HostGate::default(),
                shutdown: CancellationToken::new(),
                clock,
            }),
        };
        tokio::spawn(service.clone().run());
        service
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }

    /// The address an archived item is downloaded from, asked for when it is handed over
    /// (RD-190-13): a private repository's file is resolved with the token, every other item
    /// keeps the address it was archived under.
    pub async fn download_address(
        &self,
        subscription: &Subscription,
        url: &url::Url,
    ) -> anyhow::Result<url::Url> {
        match rd_subscription::adapter_for(&self.inner.adapters, subscription.kind) {
            Some(adapter) => adapter.download_address(subscription, url).await,
            None => Ok(url.clone()),
        }
    }

    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        (self.inner.clock)()
    }

    /// Polls one subscription now, regardless of its schedule (the "check now" action).
    pub async fn poll_now(&self, id: rd_core::SubscriptionId) -> anyhow::Result<()> {
        let Some(subscription) = self.inner.database.subscription(id).await? else {
            anyhow::bail!("subscription not found");
        };
        self.poll_contained(subscription).await;
        Ok(())
    }

    /// Falls back to defaults rather than refusing, including on a database error: this runs
    /// the poll loop, which must keep ticking. The accessor reports a malformed blob.
    async fn settings(&self) -> SubscriptionSettings {
        self.inner
            .database
            .service_settings_or_default()
            .await
            .unwrap_or_default()
    }

    async fn run(self) {
        loop {
            let settings = self.settings().await;
            let tick = std::time::Duration::from_secs(u64::from(
                settings.subscription_tick_seconds.clamp(10, 3_600),
            ));
            tokio::select! {
                () = self.inner.shutdown.cancelled() => return,
                () = tokio::time::sleep(tick) => {}
            }
            if !settings.subscription_enabled {
                continue;
            }
            if let Err(error) = self.tick(&settings).await {
                tracing::warn!(%error, "subscription poll cycle failed");
            }
        }
    }

    async fn tick(&self, settings: &SubscriptionSettings) -> anyhow::Result<()> {
        let now = self.now();
        let mut due = Vec::new();
        for subscription in self.inner.database.due_subscriptions(now).await? {
            if !self.arm(&subscription, now).await {
                due.push(subscription);
            }
        }
        if due.is_empty() {
            return Ok(());
        }
        // Bounded concurrency: a failing source must not hold up the others, but a hundred
        // subscriptions must not all open a connection at once either.
        let permits = settings.subscription_max_parallel.clamp(1, 8) as usize;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(permits));
        let mut tasks = Vec::with_capacity(due.len());
        for subscription in due {
            let permit = Arc::clone(&semaphore).acquire_owned().await?;
            let service = self.clone();
            tasks.push(tokio::spawn(async move {
                let _permit = permit;
                service.poll_contained(subscription).await;
            }));
        }
        for task in tasks {
            if let Err(error) = task.await {
                tracing::warn!(%error, "subscription poll task failed");
            }
        }
        Ok(())
    }

    /// Polls one subscription in a task of its own, so a panic inside it fails that
    /// subscription's run and the caller carries on.
    ///
    /// The run is booked here as well (audit 1.9.1, INTAKE-02): a panic skips `finish`, and a
    /// subscription without a finished run stays due, is polled again every cycle, panics
    /// again and never shows why.
    async fn poll_contained(&self, subscription: Subscription) {
        let started_at = self.now();
        let service = self.clone();
        let polled = subscription.clone();
        let task = tokio::spawn(async move { service.poll_one(polled).await });
        let Err(error) = task.await else { return };
        tracing::warn!(
            subscription = %subscription.name,
            %error,
            "subscription poll task failed"
        );
        if error.is_panic() {
            self.finish(
                &subscription,
                started_at,
                Err(anyhow::anyhow!(POLL_PANICKED)),
                spread_seed(&subscription),
            )
            .await;
        }
    }
}

/// The error a panicked poll leaves on its run: a stable code the interface translates.
pub(crate) const POLL_PANICKED: &str = "subscription.poll_panicked";

/// The id is the spread key, so subscriptions created in the same minute do not poll in the
/// same second forever.
fn spread_seed(subscription: &Subscription) -> u64 {
    subscription.id.into_uuid().as_u128() as u64
}

/// The crash point between archiving a poll's accepted items and handing them to the
/// LinkGrabber (RD-190-13, `crates/rd-core/recovery-matrix.md`).
fn after_items_archived() -> anyhow::Result<()> {
    rd_core::failpoint!("subscription.after_items_archived", || anyhow::anyhow!(
        "crash point: the items are archived and none is handed over"
    ));
    Ok(())
}
