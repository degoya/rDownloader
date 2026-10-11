//! The commands of `writer/subscriptions.rs`.

use super::Reply;

/// The commands `Writer::handle_subscriptions` applies.
pub(crate) enum SubscriptionsCommand {
    /// Subscriptions (RD-080-07).
    CreateSubscription {
        input: Box<crate::subscription_store::NewSubscription>,
        reply: Reply<rd_core::Subscription>,
    },
    UpdateSubscription {
        id: rd_core::SubscriptionId,
        input: Box<crate::subscription_store::NewSubscription>,
        /// The subscription, plus the secret reference the edit replaced, if any.
        reply: Reply<(rd_core::Subscription, Option<String>)>,
    },
    SetSubscriptionEnabled {
        id: rd_core::SubscriptionId,
        enabled: bool,
        reply: Reply<rd_core::Subscription>,
    },
    DeleteSubscription {
        id: rd_core::SubscriptionId,
        /// The secret reference to drop from the vault, if the subscription had one.
        reply: Reply<Option<String>>,
    },
    RecordSubscriptionItems {
        subscription_id: rd_core::SubscriptionId,
        items: Vec<crate::subscription_store::NewSubscriptionItem>,
        /// Only the rows this call created, the rest were already archived; and the archive
        /// password of every row it wrote, for the vault (RD-190-04).
        reply: Reply<(
            Vec<rd_core::SubscriptionItem>,
            crate::subscription_store::ItemPasswords,
        )>,
    },
    SetSubscriptionItemState {
        id: rd_core::SubscriptionItemId,
        state: rd_core::SubscriptionItemState,
        reply: Reply<()>,
    },
    SetPendingSubscriptionItemsState {
        ids: Vec<rd_core::SubscriptionItemId>,
        state: rd_core::SubscriptionItemState,
        reply: Reply<u64>,
    },
    ClearSubscriptionHistory {
        id: rd_core::SubscriptionId,
        reply: Reply<rd_core::SubscriptionHistoryClearResponse>,
    },
    /// Times a scheduled subscription's first run (RD-130-19); `false` when it already had one.
    ArmSubscription {
        id: rd_core::SubscriptionId,
        next_run_at: chrono::DateTime<chrono::Utc>,
        reply: Reply<bool>,
    },
    /// One batch of the archive's compaction (RD-1240-35); replies how many items went.
    CompactSubscriptionItems {
        before: chrono::DateTime<chrono::Utc>,
        reply: Reply<u64>,
    },
    FinishSubscriptionRun {
        subscription_id: rd_core::SubscriptionId,
        started_at: chrono::DateTime<chrono::Utc>,
        result: Box<crate::subscription_store::PollResult>,
        reply: Reply<()>,
    },
}
