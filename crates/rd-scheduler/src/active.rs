//! The files running right now, and how the global `max_active_files` limit counts them.

use std::collections::{HashMap, HashSet};

use rd_core::{DownloadId, DownloadKind};
use tokio_util::sync::CancellationToken;

use crate::StopReason;

#[derive(Default)]
pub(crate) struct ActiveState {
    pub(crate) tokens: HashMap<DownloadId, CancellationToken>,
    pub(crate) reasons: HashMap<DownloadId, StopReason>,
    /// Running files whose runner does not count against `max_active_files`.
    pub(crate) exempt: HashSet<DownloadId>,
    /// Running files of kinds whose files share one `max_active_files` slot (RD-130-22).
    pub(crate) pooled: HashMap<DownloadId, DownloadKind>,
    /// Idle files a reset, a removal or a discard is working on, with how many of them at
    /// once. The dispatcher skips them, and only the hold's own end counts down: a stop reason
    /// could be taken out by a resume or another call's `release_stop_guard` in the middle of
    /// the work (re-audit 1.9.1, RA-TR-02).
    pub(crate) held: HashMap<DownloadId, usize>,
}

impl ActiveState {
    /// Whether a dispatch pass has to leave `id` alone: it runs, it is being stopped, or a
    /// reset or removal holds it.
    pub(crate) fn untouchable(&self, id: &DownloadId) -> bool {
        self.tokens.contains_key(id) || self.reasons.contains_key(id) || self.held.contains_key(id)
    }

    /// Lets go of one hold on `id`; the last one lets the dispatcher back at it.
    pub(crate) fn release_hold(&mut self, id: &DownloadId) {
        if let Some(count) = self.held.get_mut(id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.held.remove(id);
            }
        }
    }

    /// Running files as `max_active_files` counts them: exempt ones not at all, the files
    /// of a pooled kind once for the kind.
    pub(crate) fn counted(&self) -> usize {
        let kinds: HashSet<_> = self.pooled.values().collect();
        self.tokens
            .len()
            .saturating_sub(self.exempt.len() + self.pooled.len())
            + kinds.len()
    }

    /// Whether one more file may start under a global limit of `limit`. A file of a pooled
    /// kind that already runs joins that kind's slot and needs no place of its own.
    pub(crate) fn admits(
        &self,
        kind: DownloadKind,
        exempt: bool,
        pooled: bool,
        limit: usize,
    ) -> bool {
        exempt
            || (pooled && self.pooled.values().any(|running| *running == kind))
            || self.counted() < limit
    }
}

#[cfg(test)]
mod tests {
    use rd_core::{DownloadId, DownloadKind};
    use tokio_util::sync::CancellationToken;

    use super::ActiveState;

    fn start(active: &mut ActiveState, kind: DownloadKind, exempt: bool, pooled: bool) {
        let id = DownloadId::new();
        active.tokens.insert(id, CancellationToken::new());
        if exempt {
            active.exempt.insert(id);
        } else if pooled {
            active.pooled.insert(id, kind);
        }
    }

    #[test]
    fn ordinary_files_each_take_a_place() {
        let mut active = ActiveState::default();
        start(&mut active, DownloadKind::Http, false, false);
        start(&mut active, DownloadKind::Http, false, false);
        assert_eq!(active.counted(), 2);
        assert!(active.admits(DownloadKind::Http, false, false, 3));
        assert!(!active.admits(DownloadKind::Http, false, false, 2));
    }

    /// RD-130-22: the files of one connection pool are one transfer.
    #[test]
    fn the_files_of_a_pooled_kind_share_one_place() {
        let mut active = ActiveState::default();
        start(&mut active, DownloadKind::Http, false, false);
        start(&mut active, DownloadKind::Usenet, false, true);
        start(&mut active, DownloadKind::Usenet, false, true);
        start(&mut active, DownloadKind::Usenet, false, true);
        assert_eq!(active.counted(), 2, "one HTTP file and the Usenet pool");
        // The limit is reached, yet another Usenet file joins the running ones ...
        assert!(active.admits(DownloadKind::Usenet, false, true, 2));
        // ... while an HTTP file waits, as it would behind any other transfer.
        assert!(!active.admits(DownloadKind::Http, false, false, 2));
    }

    #[test]
    fn the_first_file_of_a_pooled_kind_needs_a_place() {
        let mut active = ActiveState::default();
        start(&mut active, DownloadKind::Http, false, false);
        start(&mut active, DownloadKind::Http, false, false);
        assert!(!active.admits(DownloadKind::Usenet, false, true, 2));
        assert!(active.admits(DownloadKind::Usenet, false, true, 3));
    }

    /// RA-TR-02: two holds on one row end only with the second, and nothing else lifts them.
    #[test]
    fn a_hold_ends_with_its_last_holder() {
        let mut active = ActiveState::default();
        let id = DownloadId::new();
        *active.held.entry(id).or_default() += 1;
        *active.held.entry(id).or_default() += 1;
        active.reasons.remove(&id);
        assert!(
            active.untouchable(&id),
            "a lost stop reason does not end a hold"
        );
        active.release_hold(&id);
        assert!(active.untouchable(&id), "one holder is still at work");
        active.release_hold(&id);
        assert!(!active.untouchable(&id));
        active.release_hold(&id);
        assert!(
            active.held.is_empty(),
            "a stray release leaves nothing behind"
        );
    }

    #[test]
    fn exempt_files_take_no_place() {
        let mut active = ActiveState::default();
        start(&mut active, DownloadKind::Record, true, false);
        start(&mut active, DownloadKind::Record, true, false);
        assert_eq!(active.counted(), 0);
        assert!(active.admits(DownloadKind::Record, true, false, 0));
        assert!(active.admits(DownloadKind::Http, false, false, 1));
    }
}
