//! Per-file priority tiers, emulated on top of the engine's include/exclude selection.
//!
//! librqbit has no priority concept at all — a file is either downloaded or it is not. The
//! emulation opens one tier at a time: only the highest tier that still has unfinished
//! files is handed to the engine, and the next tier is opened once that one completes.
//!
//! Where this differs from real priorities, and why the API says so through
//! `priorities_emulated`: inside one tier the engine still downloads concurrently and in
//! its own order, and two files sharing a piece finish together regardless of their tier.

use rd_core::{DownloadId, TorrentFilePriority, TorrentJobState};

use crate::TorrentService;

/// The tier that should currently be open, given what is already finished.
///
/// `file_progress` is the engine's per-file byte counter, indexed by file index. A tier is
/// complete when every included file in it has reached its full length.
#[must_use]
pub(crate) fn open_tier(
    state: &TorrentJobState,
    file_progress: &[u64],
) -> Option<TorrentFilePriority> {
    let metadata = state.metadata.as_ref()?;
    let resolved = rd_core::resolve_plan(metadata, &state.plan);
    for tier in TorrentFilePriority::tiers() {
        let indices = resolved.indices_in_tier(tier);
        if indices.is_empty() {
            continue;
        }
        let complete = indices.iter().all(|index| {
            let Some(file) = metadata.files.iter().find(|file| file.index == *index) else {
                return true;
            };
            file_progress
                .get(*index as usize)
                .is_some_and(|done| *done >= file.length.get())
        });
        if !complete {
            return Some(tier);
        }
    }
    // Everything is done; keep the lowest tier open so nothing is dropped from the engine.
    Some(TorrentFilePriority::Low)
}

/// File indices the engine should currently download.
///
/// Without priorities this is simply the whole selection. With them it is the selection
/// restricted to the open tier, plus everything already completed so finished files are
/// not removed from the torrent.
#[must_use]
pub(crate) fn staged_selection(
    state: &TorrentJobState,
    file_progress: &[u64],
) -> Option<Vec<usize>> {
    let metadata = state.metadata.as_ref()?;
    let resolved = rd_core::resolve_plan(metadata, &state.plan);
    let included = resolved.included_indices();
    if state.plan.priorities.is_empty() {
        return Some(included.into_iter().map(|index| index as usize).collect());
    }
    let tier = open_tier(state, file_progress)?;
    let staged: Vec<usize> = included
        .into_iter()
        .filter(|index| {
            let priority = state
                .plan
                .priorities
                .get(index)
                .copied()
                .unwrap_or_default();
            if priority >= tier {
                return true;
            }
            // Already finished files stay included so the engine keeps seeding them.
            metadata
                .files
                .iter()
                .find(|file| file.index == *index)
                .is_some_and(|file| {
                    file_progress
                        .get(*index as usize)
                        .is_some_and(|done| *done >= file.length.get())
                })
        })
        .map(|index| index as usize)
        .collect();
    Some(staged)
}

impl TorrentService {
    /// Widens the engine's selection when the open priority tier has completed.
    ///
    /// Called from the runner's progress tick. Does nothing for a torrent without
    /// priorities, so the common case costs one map lookup.
    pub(crate) async fn advance_tiers(&self, id: DownloadId, file_progress: &[u64]) {
        let mut state = self.job_state(id).await;
        if state.plan.priorities.is_empty() {
            return;
        }
        let Some(tier) = open_tier(&state, file_progress) else {
            return;
        };
        if state.open_tier == Some(tier) {
            return;
        }
        let Some(staged) = staged_selection(&state, file_progress) else {
            return;
        };
        state.open_tier = Some(tier);
        self.store_job_state(id, state).await;
        let entry = self.inner.registry.read().await.get(id).cloned();
        let Some(entry) = entry else { return };
        let Ok(session) = self.session().await else {
            return;
        };
        let Some(handle) = session.get(entry.handle()) else {
            return;
        };
        let selection: std::collections::HashSet<usize> = staged.into_iter().collect();
        if let Err(error) = session.update_only_files(&handle, &selection).await {
            tracing::warn!(download_id = %id, %error, "opening the next priority tier failed");
        } else {
            tracing::debug!(download_id = %id, ?tier, "opened the next torrent priority tier");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rd_core::{
        ByteCount, TorrentFileEntry, TorrentFilePlan, TorrentFilePriority, TorrentJobState,
        TorrentMetadataInfo,
    };

    use super::{open_tier, staged_selection};

    fn state(priorities: BTreeMap<u32, TorrentFilePriority>) -> TorrentJobState {
        let files = [10_u64, 20, 30]
            .into_iter()
            .enumerate()
            .map(|(index, length)| TorrentFileEntry {
                index: u32::try_from(index).expect("small"),
                path: vec![format!("file{index}.bin")],
                length: ByteCount::new(length).expect("valid"),
            })
            .collect();
        TorrentJobState {
            metadata: Some(TorrentMetadataInfo {
                info_hash: "hash".to_owned(),
                name: "release".to_owned(),
                total_bytes: ByteCount::new(60).expect("valid"),
                piece_length: 16_384,
                piece_count: 1,
                private: false,
                files,
                trackers: Vec::new(),
                web_seeds: Vec::new(),
            }),
            plan: TorrentFilePlan {
                priorities,
                ..TorrentFilePlan::default()
            },
            ..TorrentJobState::default()
        }
    }

    #[test]
    fn without_priorities_the_whole_selection_is_staged() {
        let staged = staged_selection(&state(BTreeMap::new()), &[0, 0, 0]).expect("staged");
        assert_eq!(staged.len(), 3);
    }

    #[test]
    fn only_the_highest_unfinished_tier_is_open() {
        let state = state(BTreeMap::from([
            (0, TorrentFilePriority::High),
            (1, TorrentFilePriority::Low),
        ]));
        assert_eq!(
            open_tier(&state, &[0, 0, 0]),
            Some(TorrentFilePriority::High)
        );
        let staged = staged_selection(&state, &[0, 0, 0]).expect("staged");
        // File 2 is Normal and file 1 is Low, so neither is open yet.
        assert_eq!(staged, vec![0]);
    }

    #[test]
    fn a_finished_tier_opens_the_next_one() {
        let state = state(BTreeMap::from([
            (0, TorrentFilePriority::High),
            (1, TorrentFilePriority::Low),
        ]));
        // File 0 is complete, so the Normal tier (file 2) becomes the open one.
        assert_eq!(
            open_tier(&state, &[10, 0, 0]),
            Some(TorrentFilePriority::Normal)
        );
        let staged = staged_selection(&state, &[10, 0, 0]).expect("staged");
        assert_eq!(staged, vec![0, 2]);
    }

    #[test]
    fn the_lowest_tier_stays_open_once_everything_is_done() {
        let state = state(BTreeMap::from([(0, TorrentFilePriority::High)]));
        assert_eq!(
            open_tier(&state, &[10, 20, 30]),
            Some(TorrentFilePriority::Low)
        );
        let staged = staged_selection(&state, &[10, 20, 30]).expect("staged");
        assert_eq!(staged, vec![0, 1, 2]);
    }

    #[test]
    fn a_skipped_file_is_never_staged() {
        let state = state(BTreeMap::from([(1, TorrentFilePriority::Skip)]));
        let staged = staged_selection(&state, &[0, 0, 0]).expect("staged");
        assert!(!staged.contains(&1));
    }
}
