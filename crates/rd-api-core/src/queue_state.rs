//! The whole queue's moments (RD-1290-04): it paused, it resumed, it ran empty. Each is one
//! `queue.state` event per transition, which the notification hub turns into `queue_paused`,
//! `queue_resumed` or `queue_finished` and the automations into the triggers of the same names.
//!
//! The moments are read off the queue on the power supervisor's tick rather than announced by
//! each pauser, because the queue is paused by more than one hand and only the sum is a moment:
//! the queue pause (a person's timed pause, the automation action, the stop mark, the game mode,
//! each named in its record), the bandwidth schedule's profile that pauses downloads, and "pause
//! all", which pauses every waiting and moving file one by one. A second pause on top of the first
//! is no new moment, nor is the same pause set again. A traffic budget that runs out stays
//! `budget_exhausted`, and the holds of power, reconnects and an account's used-up traffic are not
//! a pause somebody set. The first tick after a start only learns the state: a restart is no
//! moment either.
//!
//! "Ran empty" is what the completion action waits for: nothing waiting, moving or in
//! post-processing, after something was — once, not again while it stays empty, and never while
//! the queue is paused, whose files wait without moving.

use std::collections::HashSet;

use rd_core::{DownloadFile, DownloadId, DownloadState};

/// What one tick saw.
pub(crate) struct Sample<'a> {
    /// The queue pause in force, by who set it.
    pub queue_pause: Option<rd_scheduler::PauseReason>,
    /// Whether the profile in force pauses downloads.
    pub schedule_pauses: bool,
    pub downloads: &'a [DownloadFile],
    /// Whether a package waits for or runs in post-processing.
    pub postprocessing: bool,
}

/// What the last tick concluded.
#[derive(Debug)]
struct Seen {
    /// Why the queue is paused, `None` while it is not.
    paused: Option<&'static str>,
    /// Something waits, moves or is post-processed.
    busy: bool,
    /// The files that were waiting or moving.
    moving: HashSet<DownloadId>,
    /// "Pause all" stopped every file that was moving.
    paused_by_hand: bool,
}

/// The queue's state between two ticks, so each transition is announced once.
#[derive(Debug, Default)]
pub(crate) struct QueueStateWatch {
    seen: Option<Seen>,
}

impl QueueStateWatch {
    /// The `queue.state` payloads this tick announces, in order.
    pub(crate) fn observe(&mut self, sample: &Sample<'_>) -> Vec<serde_json::Value> {
        let moving: HashSet<DownloadId> = sample
            .downloads
            .iter()
            .filter(|file| file.state.is_queued_or_working())
            .map(|file| file.id)
            .collect();
        let any_paused = sample
            .downloads
            .iter()
            .any(|file| file.state == DownloadState::Paused);
        // "Pause all" leaves no file moving and one at least of those that moved paused; it lasts
        // until something moves again or nothing paused is left. A file that finished is no
        // pause: the queue that ran empty around a file paused long before has finished.
        let stopped_by_hand = |before: &Seen| {
            !before.moving.is_empty()
                && sample.downloads.iter().any(|file| {
                    file.state == DownloadState::Paused && before.moving.contains(&file.id)
                })
        };
        let paused_by_hand = moving.is_empty()
            && any_paused
            && self
                .seen
                .as_ref()
                .is_some_and(|before| before.paused_by_hand || stopped_by_hand(before));
        let paused = match (sample.queue_pause, sample.schedule_pauses) {
            (Some(reason), _) => Some(reason.code()),
            (None, true) => Some("schedule"),
            (None, false) => paused_by_hand.then_some("manual"),
        };
        let busy = sample.postprocessing || !moving.is_empty();
        let now = Seen {
            paused,
            busy,
            moving,
            paused_by_hand,
        };
        let Some(before) = self.seen.replace(now) else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        match (before.paused, paused) {
            (None, Some(reason)) => {
                changes.push(serde_json::json!({ "state": "paused", "reason": reason }));
            }
            (Some(_), None) => changes.push(serde_json::json!({ "state": "resumed" })),
            _ => {}
        }
        if before.busy && !busy && paused.is_none() {
            changes.push(serde_json::json!({ "state": "finished" }));
        }
        changes
    }
}

#[cfg(test)]
#[path = "queue_state_tests.rs"]
mod tests;
