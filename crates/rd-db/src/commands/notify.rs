//! The commands of `writer/notify.rs`.

use super::Reply;

/// The commands `Writer::handle_notify` applies.
pub(crate) enum NotifyCommand {
    UpsertNotificationTarget {
        id: Option<rd_core::NotificationTargetId>,
        input: crate::notify_store::NewNotificationTarget,
        reply: Reply<rd_notify::NotificationTarget>,
    },
    DeleteNotificationTarget {
        id: rd_core::NotificationTargetId,
        /// The vault reference of the removed target, so the caller can drop the secret.
        reply: Reply<Option<String>>,
    },
    UpsertAutomation {
        id: Option<rd_core::AutomationId>,
        input: crate::automation_store::NewAutomation,
        reply: Reply<rd_automation::Automation>,
    },
    SetAutomationEnabled {
        id: rd_core::AutomationId,
        enabled: bool,
        reply: Reply<rd_automation::Automation>,
    },
    DeleteAutomation {
        id: rd_core::AutomationId,
        reply: Reply<()>,
    },
    QueueAutomationRun {
        input: crate::automation_store::NewRun,
        reply: Reply<bool>,
    },
    RecordAutomationAttempt {
        id: rd_core::AutomationRunId,
        state: rd_automation::RunState,
        action_index: u32,
        attempt: u32,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        message: Option<String>,
        reply: Reply<()>,
    },
    RecoverAutomationRuns {
        reply: Reply<u64>,
    },
    UpsertNotificationRule {
        id: Option<rd_core::NotificationRuleId>,
        input: crate::notify_store::NewNotificationRule,
        reply: Reply<rd_notify::NotificationRule>,
    },
    DeleteNotificationRule {
        id: rd_core::NotificationRuleId,
        reply: Reply<()>,
    },
    QueueNotificationDelivery {
        input: crate::notify_store::NewDelivery,
        reply: Reply<bool>,
    },
    /// Queues an operational notice's deliveries, each at most once per key (RD-190-19).
    QueueNotificationNotice {
        deliveries: Vec<crate::notify_store::NewDelivery>,
        reply: Reply<u64>,
    },
    RecordNotificationAttempt {
        id: rd_core::NotificationDeliveryId,
        state: rd_notify::DeliveryState,
        attempt: u32,
        next_attempt_at: Option<chrono::DateTime<chrono::Utc>>,
        response_status: Option<u16>,
        response_excerpt: Option<String>,
        reply: Reply<()>,
    },
    /// Empties the delivery history and reports how many rows went; what the worker still
    /// owes an attempt stays (RD-130-08).
    ClearNotificationDeliveries {
        reply: Reply<u64>,
    },
    /// Deletes the deliveries still queued or retrying, which cancels those notifications,
    /// and reports how many went (RD-170-11).
    DiscardPendingNotificationDeliveries {
        reply: Reply<u64>,
    },
}
