//! Database facade for subscriptions, their items and runs (RD-080-07).

use anyhow::Result;

use crate::{Database, archive_password::PasswordTable};

impl Database {
    pub async fn list_subscriptions(&self) -> Result<Vec<rd_core::Subscription>> {
        crate::subscription_store::list(&self.readers).await
    }

    pub async fn subscription(
        &self,
        id: rd_core::SubscriptionId,
    ) -> Result<Option<rd_core::Subscription>> {
        crate::subscription_store::get(&self.readers, id).await
    }

    /// Subscriptions whose next poll is due at `now`, oldest first.
    pub async fn due_subscriptions(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<rd_core::Subscription>> {
        crate::subscription_store::due(&self.readers, now).await
    }

    pub async fn subscription_item_page(
        &self,
        id: rd_core::SubscriptionId,
        state: Option<rd_core::SubscriptionItemState>,
        limit: i64,
        offset: i64,
    ) -> Result<rd_core::SubscriptionItemPage> {
        let mut page =
            crate::subscription_store::item_page(&self.readers, id, state, limit, offset).await?;
        self.reveal_archive_passwords(&mut page.items).await;
        Ok(page)
    }

    pub async fn subscription_review_summary(&self) -> Result<rd_core::SubscriptionReviewSummary> {
        crate::subscription_store::review_summary(&self.readers).await
    }

    pub async fn pending_subscription_item_ids(
        &self,
        id: rd_core::SubscriptionId,
    ) -> Result<Vec<rd_core::SubscriptionItemId>> {
        crate::subscription_store::pending_item_ids(&self.readers, id).await
    }

    /// One subscription item by id.
    pub async fn subscription_item(
        &self,
        id: rd_core::SubscriptionItemId,
    ) -> Result<Option<rd_core::SubscriptionItem>> {
        let mut item = crate::subscription_store::item(&self.readers, id).await?;
        if let Some(item) = &mut item {
            self.reveal_archive_passwords(std::slice::from_mut(item))
                .await;
        }
        Ok(item)
    }

    /// Whether `url` is still in the LinkGrabber or the download list — the intake's own
    /// duplicate test, asked before a subscription item is queued again (RD-1150-04).
    pub async fn address_in_collector_or_queue(&self, url: &url::Url) -> Result<bool> {
        let taken = sqlx::query_scalar::<_, i64>(crate::collector_store::ADDRESS_TAKEN)
            .bind(url.as_str())
            .bind(url.as_str())
            .fetch_one(&self.readers)
            .await?;
        Ok(taken != 0)
    }

    pub async fn subscription_runs(
        &self,
        id: rd_core::SubscriptionId,
        limit: i64,
    ) -> Result<Vec<rd_core::SubscriptionRun>> {
        crate::subscription_store::runs(&self.readers, id, limit).await
    }

    /// Whether the subscription has archived an item under `key` (RD-1150-05).
    pub async fn subscription_knows_item(
        &self,
        id: rd_core::SubscriptionId,
        key: &str,
    ) -> Result<bool> {
        crate::subscription_store::knows_item(&self.readers, id, key).await
    }

    pub async fn create_subscription(
        &self,
        input: crate::subscription_store::NewSubscription,
    ) -> Result<rd_core::Subscription> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::CreateSubscription {
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    /// Applies an edit and returns the subscription plus any secret reference it replaced,
    /// which the caller removes from the vault.
    pub async fn update_subscription(
        &self,
        id: rd_core::SubscriptionId,
        input: crate::subscription_store::NewSubscription,
    ) -> Result<(rd_core::Subscription, Option<String>)> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::UpdateSubscription {
                id,
                input: Box::new(input),
                reply,
            }
        })
        .await
    }

    pub async fn set_subscription_enabled(
        &self,
        id: rd_core::SubscriptionId,
        enabled: bool,
    ) -> Result<rd_core::Subscription> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::SetSubscriptionEnabled { id, enabled, reply }
        })
        .await
    }

    /// Deletes a subscription and its archive, returning its secret reference if it had one.
    pub async fn delete_subscription(&self, id: rd_core::SubscriptionId) -> Result<Option<String>> {
        let secret_ref = crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::DeleteSubscription { id, reply }
        })
        .await?;
        // The archive's passwords went with its rows (RD-190-04).
        self.sweep_archive_passwords().await;
        Ok(secret_ref)
    }

    /// Archives what a poll found and returns only the rows that were new.
    pub async fn record_subscription_items(
        &self,
        subscription_id: rd_core::SubscriptionId,
        items: Vec<crate::subscription_store::NewSubscriptionItem>,
    ) -> Result<Vec<rd_core::SubscriptionItem>> {
        let (created, passwords) = crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::RecordSubscriptionItems {
                subscription_id,
                items,
                reply,
            }
        })
        .await?;
        // An unchanged password is skipped, so a feed that lists the same release on every poll
        // writes nothing to the vault after the first (RD-190-04). A failure costs the archived
        // row its password, not the poll: the new items still carry theirs to the intake.
        if let Err(error) = self
            .store_archive_passwords(
                PasswordTable::SubscriptionItems,
                passwords
                    .into_iter()
                    .map(|(id, password)| (id, Some(password)))
                    .collect(),
            )
            .await
        {
            tracing::warn!(%error, %subscription_id, "the archive passwords of a poll could not be put in the vault");
        }
        Ok(created)
    }

    pub async fn set_subscription_item_state(
        &self,
        id: rd_core::SubscriptionItemId,
        state: rd_core::SubscriptionItemState,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::SetSubscriptionItemState { id, state, reply }
        })
        .await
    }

    pub async fn set_pending_subscription_items_state(
        &self,
        ids: Vec<rd_core::SubscriptionItemId>,
        state: rd_core::SubscriptionItemState,
    ) -> Result<u64> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::SetPendingSubscriptionItemsState {
                ids,
                state,
                reply,
            }
        })
        .await
    }

    pub async fn clear_subscription_history(
        &self,
        id: rd_core::SubscriptionId,
    ) -> Result<rd_core::SubscriptionHistoryClearResponse> {
        let cleared = crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::ClearSubscriptionHistory { id, reply }
        })
        .await?;
        self.sweep_archive_passwords().await;
        Ok(cleared)
    }

    /// Gives a scheduled subscription that was never timed its first due time (RD-130-19).
    ///
    /// Returns whether the row was armed; one that already has a next run is left alone.
    pub async fn arm_subscription(
        &self,
        id: rd_core::SubscriptionId,
        next_run_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::ArmSubscription {
                id,
                next_run_at,
                reply,
            }
        })
        .await
    }

    pub async fn finish_subscription_run(
        &self,
        subscription_id: rd_core::SubscriptionId,
        started_at: chrono::DateTime<chrono::Utc>,
        result: crate::subscription_store::PollResult,
    ) -> Result<()> {
        crate::writer::request(&self.writer, |reply| {
            crate::commands::SubscriptionsCommand::FinishSubscriptionRun {
                subscription_id,
                started_at,
                result: Box::new(result),
                reply,
            }
        })
        .await
    }
}
