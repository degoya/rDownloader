//! `notifications.json` and `queue.json`: what became of the notifications and the packages
//! (RD-1291-04).
//!
//! A report like "the package-finished message never arrives" needs three answers the other
//! entries cannot give: which rules exist and what they listen for, what happened to the
//! deliveries they queued, and in which state the packages ended. Both entries answer them by
//! ids, states, stages, events, codes and counts only. The rows come from `rd-db`, whose reads
//! never select a title, a text, an endpoint, a folder or a name; this module bounds them, says
//! the bounds in the file, and passes every string through the same text redaction as the
//! configuration, so a stray URL in an error code still loses its credential.

use anyhow::{Context, Result};
use rd_core::redact_text;
use rd_db::{
    DeliveryCount, DiagnosticDelivery, DiagnosticRule, DiagnosticTarget, DownloadCount,
    PackageTransition, ValueCount,
};
use serde::Serialize;

use crate::{
    bundle::{BundleInput, InventoryEntry, json},
    notes::{self, Note},
};

/// Days the per-event delivery counts of `notifications.json` cover.
pub const DELIVERY_WINDOW_DAYS: i64 = 7;
/// Deliveries `notifications.json` lists at most, newest first.
pub const RECENT_DELIVERIES_LIMIT: usize = 50;
/// Package transitions `queue.json` lists at most, newest first.
pub const RECENT_TRANSITIONS_LIMIT: usize = 50;

#[derive(Serialize)]
struct Notifications<'a> {
    targets: &'a [DiagnosticTarget],
    rules: &'a [DiagnosticRule],
    window_days: i64,
    delivery_counts: &'a [DeliveryCount],
    recent_limit: usize,
    recent_deliveries: &'a [DiagnosticDelivery],
}

#[derive(Serialize)]
struct Queue<'a> {
    packages_by_state: &'a [ValueCount],
    packages_by_stage: &'a [ValueCount],
    transition_limit: usize,
    recent_transitions: &'a [PackageTransition],
    downloads: &'a [DownloadCount],
}

/// Every string of `value` through the text redaction; keys, numbers and booleans as they are.
fn redact_strings(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(redact_text(&text)),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(redact_strings).collect())
        }
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .into_iter()
                .map(|(key, inner)| (key, redact_strings(inner)))
                .collect(),
        ),
        other => other,
    }
}

fn redacted<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let value = serde_json::to_value(value).context("render bundle entry")?;
    json(&redact_strings(value))
}

fn redactions(code: &str) -> Vec<Note> {
    vec![
        Note::new(code),
        Note::new(notes::REDACTION_CONFIGURATION_TEXT),
    ]
}

/// The two entries, rendered, in archive order.
pub(crate) fn entries(input: &BundleInput) -> Result<Vec<(InventoryEntry, Vec<u8>)>> {
    let notifications = &input.notifications;
    let deliveries = &notifications.recent_deliveries[..notifications
        .recent_deliveries
        .len()
        .min(RECENT_DELIVERIES_LIMIT)];
    let queue = &input.queue;
    let transitions =
        &queue.recent_transitions[..queue.recent_transitions.len().min(RECENT_TRANSITIONS_LIMIT)];
    Ok(vec![
        (
            InventoryEntry {
                id: "notifications".to_owned(),
                path: "notifications.json".to_owned(),
                kind: "json".to_owned(),
                description: Note::new(notes::ENTRY_NOTIFICATIONS),
                items: (notifications.targets.len()
                    + notifications.rules.len()
                    + notifications.delivery_counts.len()
                    + deliveries.len()) as u64,
                redactions: redactions(notes::REDACTION_NOTIFICATIONS),
            },
            redacted(&Notifications {
                targets: &notifications.targets,
                rules: &notifications.rules,
                window_days: DELIVERY_WINDOW_DAYS,
                delivery_counts: &notifications.delivery_counts,
                recent_limit: RECENT_DELIVERIES_LIMIT,
                recent_deliveries: deliveries,
            })?,
        ),
        (
            InventoryEntry {
                id: "queue".to_owned(),
                path: "queue.json".to_owned(),
                kind: "json".to_owned(),
                description: Note::new(notes::ENTRY_QUEUE),
                items: (queue.packages_by_state.len()
                    + queue.packages_by_stage.len()
                    + transitions.len()
                    + queue.downloads.len()) as u64,
                redactions: redactions(notes::REDACTION_QUEUE),
            },
            redacted(&Queue {
                packages_by_state: &queue.packages_by_state,
                packages_by_stage: &queue.packages_by_stage,
                transition_limit: RECENT_TRANSITIONS_LIMIT,
                recent_transitions: transitions,
                downloads: &queue.downloads,
            })?,
        ),
    ])
}
