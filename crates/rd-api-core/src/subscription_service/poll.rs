//! One poll of one subscription: the request, the archive, the hand-over and the run record.

use super::*;

impl SubscriptionService {
    pub(super) async fn poll_one(&self, subscription: Subscription) {
        let started_at = self.now();
        let settings = self.settings().await;
        let seed = spread_seed(&subscription);

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

        let cap = items_per_poll(subscription.kind);
        if outcome.items.len() > cap {
            // The rest is not lost: the next poll sees them again, since nothing was written.
            tracing::warn!(
                subscription = %subscription.name,
                found,
                limit = cap,
                "more results than one poll takes; the remainder waits for the next poll"
            );
        }
        let records = item_records(&subscription, &outcome.items, primed, now);

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
            // A stop here leaves the accepted items archived as pending and none of them in
            // the LinkGrabber: the next poll does not hand them over a second time, and the
            // review list still offers them (RD-190-13, recovery matrix).
            if let Err(error) = after_items_archived() {
                self.finish(&subscription, started_at, Err(error), seed)
                    .await;
                return;
            }
            self.queue_accepted(adapter, &subscription, &created).await;
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
                paused_until: outcome.paused_until,
            }),
            seed,
        )
        .await;
    }

    /// Hands the accepted items of an auto-queue poll to the LinkGrabber, one batch per
    /// category, and marks the ones it took as queued.
    async fn queue_accepted(
        &self,
        adapter: &Arc<dyn SourceAdapter>,
        subscription: &Subscription,
        created: &[rd_core::SubscriptionItem],
    ) {
        let queueable: Vec<&rd_core::SubscriptionItem> = created
            .iter()
            .filter(|item| item.state == SubscriptionItemState::Pending)
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
            let mut handed = Vec::new();
            for (category, items) in by_category {
                handed.extend(
                    self.hand_to_intake(adapter, subscription, &items, category)
                        .await,
                );
            }
            if !handed.is_empty()
                && let Err(error) = self
                    .inner
                    .database
                    .set_pending_subscription_items_state(handed, SubscriptionItemState::Queued)
                    .await
            {
                tracing::warn!(
                    subscription = %subscription.name,
                    %error,
                    "handed-over subscription items could not be marked queued"
                );
            }
        }
    }

    /// Hands accepted items to the ordinary LinkGrabber intake and returns the ones it took.
    ///
    /// An item whose download address cannot be had right now — a private repository's file
    /// the forge will not resolve — is left out and stays pending, rather than handing the
    /// LinkGrabber an address it cannot use.
    async fn hand_to_intake(
        &self,
        adapter: &Arc<dyn SourceAdapter>,
        subscription: &Subscription,
        items: &[&rd_core::SubscriptionItem],
        category_id: Option<rd_core::CategoryId>,
    ) -> Vec<rd_core::SubscriptionItemId> {
        let mut resolved = Vec::with_capacity(items.len());
        for item in items {
            match adapter.download_address(subscription, &item.url).await {
                Ok(url) => resolved.push((*item, url)),
                Err(error) => tracing::warn!(
                    subscription = %subscription.name,
                    item = %item.title,
                    error = %rd_core::redact_text(&error.to_string()),
                    "subscription item has no download address now; it stays for review"
                ),
            }
        }
        if resolved.is_empty() {
            return Vec::new();
        }
        // What the feed said each address is, so an indexer's API call is imported rather
        // than fetched as a file, and what it called the item, so the release keeps its name
        // instead of the indexer's endpoint.
        let links: Vec<crate::collector_intake::DeclaredLink> = resolved
            .iter()
            .map(|(item, url)| crate::collector_intake::DeclaredLink {
                url: url.clone(),
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
        // carries on, its items still pending. The review action propagates the same error.
        match hand_urls_to_intake(&intake, &subscription.name, links, category_id).await {
            Ok(()) => resolved.into_iter().map(|(item, _)| item.id).collect(),
            Err(error) => {
                tracing::warn!(
                    subscription = %subscription.name,
                    error = %error.message(),
                    "subscription items could not be handed to intake"
                );
                Vec::new()
            }
        }
    }

    /// Writes the run, the next due time and the failure counter in one place.
    pub(super) async fn finish(
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
                // A source that asked for a pause (RD-190-13) gets it, interval or not.
                next_run_at: scheduled
                    .unwrap_or_else(|| rd_subscription::next_success(now, interval, seed))
                    .max(counts.paused_until.unwrap_or(now)),
                consecutive_failures: 0,
                etag: counts.etag,
                last_modified: counts.last_modified,
            },
            Err(error)
                if error
                    .downcast_ref::<rd_subscription::RateLimited>()
                    .is_some() =>
            {
                // Waited out rather than backed off (RD-190-13): the source named its time, the
                // failure count stays where it was, and the log says it once, quietly.
                let until = error
                    .downcast_ref::<rd_subscription::RateLimited>()
                    .map_or(now, |limited| limited.until);
                tracing::info!(subscription = %subscription.name, %until, "rate limited");
                PollResult {
                    found: 0,
                    accepted: 0,
                    skipped: 0,
                    error: Some(error.to_string()),
                    next_run_at: until,
                    consecutive_failures: subscription.consecutive_failures,
                    etag: None,
                    last_modified: None,
                }
            }
            Err(error) => {
                let failures = subscription.consecutive_failures.saturating_add(1);
                // Redacted: a poll URL can carry an indexer API key, and this message is
                // shown in the UI and stored on the row.
                // A key the vault holds but cannot open is recorded by its code, whatever
                // context the poll wrapped it in (RD-1240-36).
                let message = match rd_secrets::find_unreadable(&error) {
                    Some(unreadable) => unreadable.to_string(),
                    None => rd_core::redact_text(&error.to_string()),
                };
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

/// Most items one poll of `kind` archives.
///
/// An indexer poll is the exception (RD-1150-05): it stops paging where it meets its archive, so
/// an entry it brought and this cap left out would sit below the next poll's stopping point and
/// never come back. Everything one indexer poll can bring is taken.
pub(super) fn items_per_poll(kind: rd_core::SubscriptionKind) -> usize {
    match kind {
        rd_core::SubscriptionKind::Indexer => rd_subscription::MAX_INDEXER_ITEMS,
        _ => rd_core::MAX_ITEMS_PER_POLL,
    }
}

/// The archive rows of one poll's items: each accepted or skipped, with the reason why.
fn item_records(
    subscription: &Subscription,
    items: &[rd_subscription::DiscoveredItem],
    primed: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<NewSubscriptionItem> {
    let mut records = Vec::with_capacity(items.len());
    let filters = rd_subscription::PreparedFilters::new(&subscription.filters);
    for item in items.iter().take(items_per_poll(subscription.kind)) {
        // The adapter's own refusal (RD-190-13) is as final as a filter's, and stored the
        // same way.
        let decision = match item.refused {
            Some(reason) => Err(reason),
            None => filters.evaluate(
                &rd_subscription::candidate_of(item),
                subscription.backlog,
                primed,
                now,
            ),
        };
        let (state, reason) = match decision {
            // Pending even when it is about to be queued: it becomes `Queued` once the
            // LinkGrabber has it, and not a moment before.
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
    records
}

/// What a successful poll counted.
pub(super) struct PollCounts {
    pub(super) found: u32,
    pub(super) accepted: u32,
    skipped: u32,
    etag: Option<String>,
    last_modified: Option<String>,
    /// When the source asked to be asked again at the earliest (RD-190-13).
    paused_until: Option<chrono::DateTime<chrono::Utc>>,
}
