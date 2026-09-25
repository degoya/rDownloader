//! The way back from post-processing into downloading (RD-107-04).
//!
//! Recovery volumes that were postponed when the NZB was queued sit in the package as
//! `DownloadState::Skipped` rows. Nothing looks at them until the PAR2 stage reports that a
//! repair is short of blocks — and at that moment the scheduler considers the package
//! downloaded and the pipeline is already running, which is exactly the situation nothing in
//! this application could get out of before.
//!
//! The way out is deliberately built out of state that already survives a restart rather than
//! out of an in-memory wait. Releasing a volume means moving its row back to `Queued`; the
//! package goes back to `PackageState::Downloading`, which drops the post-processing hold and
//! lets the dispatcher pick the volume up like any other queued file. When the last of them
//! reaches a terminal state, the completion listener in `lib.rs` requests the package again,
//! and the whole pipeline — PAR2 first — runs from the top with the new blocks on disk. A
//! crash anywhere in between changes nothing: the rows are queued, the package is downloading,
//! and the same listener fires after the restart.
//!
//! Termination is what makes that safe to repeat. Every pass either releases at least one
//! volume (the pool of postponed rows strictly shrinks) or finds nothing left to release, and
//! the second case is the final verdict.

use std::path::Path;

use anyhow::Result;
use rd_core::{DownloadFile, DownloadState, PackageId, PostprocessKind, PostprocessState};

use crate::{Inner, par2_job::BlockShortfall};

/// Stable code for a package that is waiting for recovery volumes it asked for.
pub(crate) const AWAITING_BLOCKS: &str = "postprocess.par2_awaiting_blocks";

/// Blocks assumed for a volume whose name does not say how many it carries.
///
/// The smallest useful value on purpose: guessing high would stop the planner one volume
/// short of what the repair actually needs, and a volume too many only costs bandwidth.
const UNKNOWN_VOLUME_BLOCKS: u32 = 1;

/// What the return path decided.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Refill {
    /// Volumes are on their way; the pipeline stands down and is requested again later.
    Waiting {
        /// Rows moved from `Skipped` back to `Queued` by this pass.
        released: usize,
        /// Rows that were already on their way when this pass looked.
        pending: usize,
    },
    /// No postponed volume is left that could close the gap: the shortfall is final.
    Exhausted,
}

/// Whether a re-queued recovery volume of this package has not arrived yet.
///
/// The guard in front of the whole pipeline. Post-processing a package whose own volumes are
/// still being fetched would measure the same gap a second time and plan against data that is
/// already on its way; the completion listener requests the package again when the last of
/// them lands, so waiting costs nothing and repeating does.
#[must_use]
pub(crate) fn volumes_on_the_way(downloads: &[DownloadFile]) -> bool {
    downloads
        .iter()
        .any(|file| rd_core::is_par2_volume(&file.file_name) && is_on_the_way(file.state))
}

/// Re-queues as many postponed volumes as the measured gaps need, and no more.
///
/// Each gap is answered on its own: an index whose volumes are already on their way is left
/// alone, an index with postponed volumes that cover the gap releases them, and an index with
/// nothing left to release keeps the verdict the PAR2 stage already recorded for it.
pub(crate) async fn run(
    inner: &Inner,
    owner: &str,
    downloads: &[DownloadFile],
    shortfalls: &[BlockShortfall],
) -> Result<Refill> {
    let mut released = 0_usize;
    let mut pending = 0_usize;
    for shortfall in shortfalls {
        let Some(index_name) = file_name(&shortfall.index) else {
            continue;
        };
        let members: Vec<&DownloadFile> = downloads
            .iter()
            .filter(|file| rd_core::par2_volume_belongs_to(index_name, &file.file_name))
            .collect();
        // A volume that is already on its way answers the question this pass was going to
        // ask. Planning on top of it would order the gap covered twice.
        let on_the_way = members
            .iter()
            .filter(|file| is_on_the_way(file.state))
            .count();
        let chosen = if on_the_way > 0 {
            Vec::new()
        } else {
            plan(&members, shortfall.needed)
        };
        let mut blocks = 0_u32;
        for file in &chosen {
            inner
                .database
                .transition_download(file.id, DownloadState::Queued)
                .await?;
            blocks = blocks.saturating_add(volume_blocks(&file.file_name));
        }
        if on_the_way == 0 && chosen.is_empty() {
            // Nothing left for this index: the verdict the PAR2 stage recorded stands.
            continue;
        }
        tracing::info!(
            index = %index_name,
            needed = shortfall.needed,
            available = shortfall.available,
            released = chosen.len(),
            already_on_the_way = on_the_way,
            "waiting for postponed PAR2 volumes to close a repair gap"
        );
        released += chosen.len();
        pending += on_the_way;
        await_volumes(
            inner,
            owner,
            &shortfall.index,
            on_the_way + chosen.len(),
            blocks,
        )
        .await?;
    }
    if released == 0 && pending == 0 {
        return Ok(Refill::Exhausted);
    }
    Ok(Refill::Waiting { released, pending })
}

/// Records on one PAR2 step that it is waiting rather than broken.
///
/// The step goes back to `Queued` instead of staying `Failed`: it has not failed, it is
/// waiting for data somebody has now asked for, and `Failed` would be what the interface shows
/// for the whole length of the wait.
async fn await_volumes(
    inner: &Inner,
    owner: &str,
    index: &Path,
    volumes: usize,
    blocks: u32,
) -> Result<()> {
    let Some(source) = index.to_str() else {
        return Ok(());
    };
    crate::steps::checkpoint_coded(
        inner,
        owner,
        PostprocessKind::Par2,
        source,
        PostprocessState::Queued,
        None,
        crate::steps::Outcome {
            code: AWAITING_BLOCKS,
            params: [
                ("volumes".to_owned(), volumes.to_string()),
                ("blocks".to_owned(), blocks.to_string()),
                ("count".to_owned(), volumes.to_string()),
            ]
            .into_iter()
            .collect(),
            message: Some(format!(
                "waiting for {volumes} postponed PAR2 volume(s) carrying {blocks} block(s)"
            )),
        },
    )
    .await
}

/// Hands the package back to the queue once at least one gap is being answered.
///
/// `Downloading` is both what is true and what releases the post-processing hold: the
/// dispatcher will not start the volumes it has just been handed while it still believes a
/// package is being post-processed.
pub(crate) async fn stand_down(inner: &Inner, package_id: PackageId, refill: Refill) -> Result<()> {
    if refill == Refill::Exhausted {
        return Ok(());
    }
    inner
        .database
        .set_package_state(
            package_id,
            rd_core::PackageState::Downloading,
            None,
            None,
            None,
        )
        .await?;
    Ok(())
}

/// The smallest selection of postponed volumes whose blocks cover `needed`.
///
/// SABnzbd's `get_extra_blocks`: sort by the block count a volume announces and take from the
/// small end until the gap is covered. Fetching the one big volume instead would close the
/// same gap with far more bytes. An empty result means the postponed volumes together cannot
/// cover the gap, and fetching them would be traffic spent on a repair that still fails.
fn plan<'a>(members: &[&'a DownloadFile], needed: u32) -> Vec<&'a DownloadFile> {
    let mut postponed: Vec<&DownloadFile> = members
        .iter()
        .copied()
        .filter(|file| file.state == DownloadState::Skipped)
        .collect();
    postponed.sort_by_key(|file| (volume_blocks(&file.file_name), file.file_name.clone()));
    let total = postponed.iter().fold(0_u32, |total, file| {
        total.saturating_add(volume_blocks(&file.file_name))
    });
    if total < needed {
        return Vec::new();
    }
    let mut covered = 0_u32;
    let mut chosen = Vec::new();
    for file in postponed {
        if covered >= needed {
            break;
        }
        covered = covered.saturating_add(volume_blocks(&file.file_name));
        chosen.push(file);
    }
    chosen
}

/// Whether a row is going to arrive on its own, so nothing has to be planned for it.
fn is_on_the_way(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Queued
            | DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::RetryWait
            | DownloadState::Verifying
            | DownloadState::Repairing
    )
}

fn volume_blocks(file_name: &str) -> u32 {
    rd_core::par2_volume_blocks(file_name).unwrap_or(UNKNOWN_VOLUME_BLOCKS)
}

fn file_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|value| value.to_str())
}

#[cfg(test)]
mod tests {
    use rd_core::{DownloadFile, DownloadState};

    fn volume(name: &str, state: DownloadState) -> DownloadFile {
        DownloadFile {
            id: rd_core::DownloadId::new(),
            package_id: rd_core::PackageId::new(),
            source: format!("nzb://fixture/{name}").parse().expect("URL"),
            file_name: name.to_owned(),
            state,
            total_bytes: None,
            committed_bytes: rd_core::ByteCount::default(),
            retry_count: 0,
            next_retry_at: None,
            expected_checksum: None,
            computed_checksum: None,
            last_error: None,
            account_id: None,
            proxy_profile_id: None,
            remote_credential_id: None,
            mirror_group: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            recording: None,
            position: 0,
            kind: rd_core::DownloadKind::Usenet,
            // These fixtures are PAR2 volumes by construction (RD-107-10's marking).
            recovery: rd_core::is_recovery_volume(name),
            nzb_file_id: None,
            media: None,
            enrichment: Vec::new(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    /// Exactly as many volumes as the gap needs, smallest first — SABnzbd's `get_extra_blocks`.
    #[test]
    fn the_plan_covers_the_gap_and_stops_there() {
        let files = [
            volume("release.vol000+01.par2", DownloadState::Skipped),
            volume("release.vol001+02.par2", DownloadState::Skipped),
            volume("release.vol003+16.par2", DownloadState::Skipped),
        ];
        let members: Vec<&DownloadFile> = files.iter().collect();

        let chosen = super::plan(&members, 3);

        assert_eq!(
            chosen
                .iter()
                .map(|file| file.file_name.as_str())
                .collect::<Vec<_>>(),
            vec!["release.vol000+01.par2", "release.vol001+02.par2"],
            "the 16-block volume would have closed the same gap with eight times the bytes"
        );
    }

    /// A set that cannot cover the gap is not fetched at all: the repair fails either way.
    #[test]
    fn a_set_that_cannot_cover_the_gap_is_left_alone() {
        let files = [
            volume("release.vol000+01.par2", DownloadState::Skipped),
            volume("release.vol001+02.par2", DownloadState::Skipped),
        ];
        let members: Vec<&DownloadFile> = files.iter().collect();

        assert!(super::plan(&members, 9).is_empty());
    }

    /// Only postponed rows are candidates; one that already arrived is not re-queued.
    #[test]
    fn volumes_that_are_not_postponed_are_not_candidates() {
        let files = [
            volume("release.vol000+01.par2", DownloadState::Completed),
            volume("release.vol001+02.par2", DownloadState::Skipped),
        ];
        let members: Vec<&DownloadFile> = files.iter().collect();

        let chosen = super::plan(&members, 1);

        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].file_name, "release.vol001+02.par2");
    }
}
