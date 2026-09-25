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
mod intake;
mod scheduled;
#[cfg(test)]
mod tests;

pub use adapters::{HttpFeedFetcher, SandboxScriptRunner, SharedSiteRules, VaultSecretResolver};
pub(crate) use intake::{SubscriptionIntake, hand_urls_to_intake};

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

    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        (self.inner.clock)()
    }

    /// Polls one subscription now, regardless of its schedule (the "check now" action).
    pub async fn poll_now(&self, id: rd_core::SubscriptionId) -> anyhow::Result<()> {
        let Some(subscription) = self.inner.database.subscription(id).await? else {
            anyhow::bail!("subscription not found");
        };
        self.poll_one(subscription).await;
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
                service.poll_one(subscription).await;
            }));
        }
        for task in tasks {
            // A panicking poll is contained: it fails its own subscription and the cycle
            // carries on, rather than taking the loop down with it.
            if let Err(error) = task.await {
                tracing::warn!(%error, "subscription poll task failed");
            }
        }
        Ok(())
    }

    async fn poll_one(&self, subscription: Subscription) {
        let started_at = self.now();
        let settings = self.settings().await;
        // The id is the spread key, so subscriptions created in the same minute do not poll
        // in the same second forever.
        let seed = subscription.id.into_uuid().as_u128() as u64;

        // A stored expression that names no time fails the run with its reason, rather than
        // running on an interval nobody chose.
        if let Some(expression) = &subscription.schedule
            && let Err(error) =
                rd_subscription::next_scheduled(expression, started_at, &chrono::Local)
        {
            self.finish(&subscription, started_at, Err(error), seed)
                .await;
            return;
        }

        let Some(adapter) = rd_subscription::adapter_for(&self.inner.adapters, subscription.kind)
        else {
            self.finish(
                &subscription,
                started_at,
                Err(anyhow::anyhow!("no adapter for this subscription kind")),
                seed,
            )
            .await;
            return;
        };

        let timeout = std::time::Duration::from_secs(u64::from(
            settings.subscription_poll_timeout_seconds.clamp(10, 3_600),
        ));
        // Held across the request only. Several subscriptions against one indexer -- one per
        // category, which is how they are actually configured -- become due together, and
        // arriving as four clients at once is a good way to be counted as abuse. Outside the
        // timeout on purpose: waiting for a turn is not the indexer being slow, and a poll
        // that waited must still get its full budget.
        let _host = self.inner.hosts.enter(&subscription.url).await;
        let outcome = match tokio::time::timeout(timeout, adapter.poll(&subscription)).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => {
                self.finish(&subscription, started_at, Err(error), seed)
                    .await;
                return;
            }
            Err(_) => {
                self.finish(
                    &subscription,
                    started_at,
                    Err(anyhow::anyhow!("poll timed out")),
                    seed,
                )
                .await;
                return;
            }
        };

        let found = u32::try_from(outcome.items.len()).unwrap_or(u32::MAX);
        let now = self.now();
        // A source without a history (RD-130-19) is primed from its first run: a script prints
        // what it was written to find now, and nobody set one up to have its output recorded
        // as the past.
        let primed = subscription.primed || !subscription.kind.has_backlog();
        // A backlog collection is always reviewed, whatever the mode says: the point of
        // asking for the history is to look at it, not to queue a decade of uploads.
        let review_only = !primed && matches!(subscription.backlog, BacklogPolicy::ReviewAll);
        let auto_queue = subscription.mode == SubscriptionMode::AutoQueue && !review_only;

        if outcome.items.len() > rd_core::MAX_ITEMS_PER_POLL {
            // The rest is not lost: the next poll sees them again, since nothing was written.
            tracing::warn!(
                subscription = %subscription.name,
                found,
                limit = rd_core::MAX_ITEMS_PER_POLL,
                "more results than one poll takes; the remainder waits for the next poll"
            );
        }
        let mut records = Vec::with_capacity(outcome.items.len());
        for item in outcome.items.iter().take(rd_core::MAX_ITEMS_PER_POLL) {
            let candidate = rd_subscription::candidate_of(item);
            let decision = rd_subscription::evaluate(
                &candidate,
                &subscription.filters,
                subscription.backlog,
                primed,
                now,
            );
            let (state, reason) = match decision {
                Ok(()) if auto_queue => (SubscriptionItemState::Queued, None),
                Ok(()) => (SubscriptionItemState::Pending, None),
                // Stored, not dropped: a rejected item that were simply not written would be
                // rediscovered on every poll forever, and the reason is what makes an
                // over-strict filter visible instead of looking like an empty channel.
                Err(reason) => (SubscriptionItemState::Skipped, Some(reason)),
            };
            records.push(NewSubscriptionItem {
                item_key: rd_subscription::key_of(item),
                title: item.title.clone(),
                url: item.url.clone(),
                published_at: item.published_at,
                duration_seconds: item.duration_seconds,
                state,
                reason,
                source_category: item.source_category.clone(),
                media_type: item.media_type.clone(),
                attributes: item.attributes.clone(),
                // Two sources, in this order (RD-101-17). The adapter reports a password only
                // when the indexer wrote a real one where the specification wants its `0`/`1`
                // flag; far more common is the SABnzbd convention in the title, which the NZB
                // import already understands but only after the title has become a file name.
                // Reading it here means a torrent hit keeps its password too.
                password: item
                    .password
                    .clone()
                    .or_else(|| rd_files::strip_password_marker(&item.title).1),
            });
        }

        // Only rows this call created come back; everything else was already archived, which
        // is what makes the whole poll safe to repeat.
        let created = match self
            .inner
            .database
            .record_subscription_items(subscription.id, records)
            .await
        {
            Ok(created) => created,
            Err(error) => {
                self.finish(&subscription, started_at, Err(error), seed)
                    .await;
                return;
            }
        };

        let accepted = created
            .iter()
            .filter(|item| item.state != SubscriptionItemState::Skipped)
            .count();
        let skipped = created.len() - accepted;

        if auto_queue {
            let queueable: Vec<&rd_core::SubscriptionItem> = created
                .iter()
                .filter(|item| item.state == SubscriptionItemState::Queued)
                .collect();
            if !queueable.is_empty() {
                // Grouped by resolved category: one intake batch per destination, because a
                // batch carries one category and mixing them would put every release in
                // whichever category happened to come first (RD-080-11).
                let mut by_category: std::collections::BTreeMap<
                    Option<rd_core::CategoryId>,
                    Vec<&rd_core::SubscriptionItem>,
                > = std::collections::BTreeMap::new();
                for item in queueable {
                    let category = subscription.category_for(item.source_category.as_deref());
                    by_category.entry(category).or_default().push(item);
                }
                for (category, items) in by_category {
                    self.hand_to_intake(&subscription, &items, category).await;
                }
            }
        }

        self.finish(
            &subscription,
            started_at,
            Ok(PollCounts {
                found,
                accepted: u32::try_from(accepted).unwrap_or(u32::MAX),
                skipped: u32::try_from(skipped).unwrap_or(u32::MAX),
                etag: outcome.etag,
                last_modified: outcome.last_modified,
            }),
            seed,
        )
        .await;
    }

    /// Hands accepted items to the ordinary LinkGrabber intake.
    async fn hand_to_intake(
        &self,
        subscription: &Subscription,
        items: &[&rd_core::SubscriptionItem],
        category_id: Option<rd_core::CategoryId>,
    ) {
        // What the feed said each address is, so an indexer's API call is imported rather
        // than fetched as a file, and what it called the item, so the release keeps its name
        // instead of the indexer's endpoint.
        let links: Vec<crate::collector_handlers::DeclaredLink> = items
            .iter()
            .map(|item| crate::collector_handlers::DeclaredLink {
                url: item.url.clone(),
                media_type: item.media_type.clone(),
                // Without the `{{secret}}` marker, which is a password and has no business
                // in a package name on somebody's screen. The archived item keeps the title
                // exactly as the indexer wrote it, because that is what identity is built
                // from; only what is handed onward is cleaned. SABnzbd names its jobs the
                // same way.
                name: Some(rd_files::strip_password_marker(&item.title).0),
                password: item.password.clone(),
                // What the indexer declared, through the `attributes.rs` gate once more
                // (RD-107-02). The stored map already passed it and `retain` is idempotent,
                // so this costs nothing and keeps the rule at the boundary it is stated for:
                // nothing that gate discards may travel further towards a plugin.
                attributes: rd_subscription::retain_attributes(&item.attributes, None).attributes,
            })
            .collect();
        let intake = SubscriptionIntake {
            database: &self.inner.database,
            link_check: &self.inner.link_check,
            media_settings: &self.inner.media_settings,
            gallery_settings: &self.inner.gallery_settings,
        };
        // The poll loop has nobody to report to, so a rejected batch is logged and the run
        // carries on. The review action propagates the same error instead.
        if let Err(error) =
            hand_urls_to_intake(&intake, &subscription.name, links, category_id).await
        {
            tracing::warn!(
                subscription = %subscription.name,
                error = %error.message(),
                "subscription items could not be handed to intake"
            );
        }
    }

    /// Writes the run, the next due time and the failure counter in one place.
    async fn finish(
        &self,
        subscription: &Subscription,
        started_at: chrono::DateTime<chrono::Utc>,
        outcome: Result<PollCounts, anyhow::Error>,
        seed: u64,
    ) {
        let now = self.now();
        let interval = subscription.effective_interval();
        // The next time a schedule names, when the subscription has one that names any.
        let scheduled = subscription.schedule.as_deref().and_then(|expression| {
            rd_subscription::next_scheduled(expression, now, &chrono::Local).ok()
        });
        let result = match outcome {
            Ok(counts) => PollResult {
                found: counts.found,
                accepted: counts.accepted,
                skipped: counts.skipped,
                error: None,
                next_run_at: scheduled
                    .unwrap_or_else(|| rd_subscription::next_success(now, interval, seed)),
                consecutive_failures: 0,
                etag: counts.etag,
                last_modified: counts.last_modified,
            },
            Err(error) => {
                let failures = subscription.consecutive_failures.saturating_add(1);
                // Redacted: a poll URL can carry an indexer API key, and this message is
                // shown in the UI and stored on the row.
                let message = rd_core::redact_text(&error.to_string());
                tracing::warn!(
                    subscription = %subscription.name,
                    failures,
                    error = %message,
                    "subscription poll failed"
                );
                PollResult {
                    found: 0,
                    accepted: 0,
                    skipped: 0,
                    error: Some(message),
                    next_run_at: match scheduled {
                        Some(scheduled) => rd_subscription::next_scheduled_failure(
                            scheduled, now, interval, failures, seed,
                        ),
                        None => rd_subscription::next_failure(now, interval, failures, seed),
                    },
                    consecutive_failures: failures,
                    etag: None,
                    last_modified: None,
                }
            }
        };
        if let Err(error) = self
            .inner
            .database
            .finish_subscription_run(subscription.id, started_at, result)
            .await
        {
            tracing::warn!(%error, "subscription run could not be recorded");
        }
    }
}

/// What a successful poll counted.
struct PollCounts {
    found: u32,
    accepted: u32,
    skipped: u32,
    etag: Option<String>,
    last_modified: Option<String>,
}
