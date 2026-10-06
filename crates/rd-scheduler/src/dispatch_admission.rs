//! Whether a queued file may start in this dispatch pass, and the slots it starts with.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

use anyhow::Result;
use rd_core::{DownloadState, PackageId};
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::CancellationToken;

use crate::{
    ExternalRunner, SchedulerHandle, host_wait::HostClaim, mirrors, provider::ProviderSlot,
    runner::KindSlot,
};

/// What a file starts with: its runner and the slot of its kind, or, for the built-in worker,
/// its provider's permit. Both are held until the attempt ends.
pub(super) struct Slots {
    pub(super) external: Option<(Arc<dyn ExternalRunner>, KindSlot)>,
    pub(super) provider_permit: Option<OwnedSemaphorePermit>,
}

/// How a file's entry into the active set went.
pub(super) enum Claim {
    Claimed,
    /// Not now: it is untouchable or the cap is reached; the pass goes on with the next file.
    Refused,
    /// The service is shutting down: the pass ends.
    ShuttingDown,
    /// Its host has no free connection: the file stays queued, waiting for this host, and
    /// leaves its place to a file of another host (RD-1130-02).
    HostBusy(String),
}

impl SchedulerHandle {
    /// Whether nothing holds `file` back in this pass: its hoster's limit, its mirror group,
    /// its storage root or its switched-off kind.
    pub(super) async fn may_start(
        &self,
        file: &rd_core::DownloadFile,
        now: chrono::DateTime<chrono::Utc>,
        destinations: &HashMap<PackageId, PathBuf>,
        groups: &mut HashMap<PackageId, Vec<rd_core::DownloadFile>>,
        blocked_kinds: &mut Vec<rd_core::DownloadKind>,
    ) -> Result<bool> {
        // A hoster's free-download limit applies to the whole IP, so hold back its
        // other anonymous links instead of spending another wait and captcha on them.
        // Downloads backed by an account are unaffected.
        if file.account_id.is_none() && self.host_blocks.blocked_until(&file.source, now).is_some()
        {
            return Ok(false);
        }
        if file.mirror_group.is_some() && self.mirror_stands_down(file, groups).await? {
            return Ok(false);
        }
        // A storage root below its threshold holds back only its own packages; every
        // other destination keeps downloading.
        if let Some(destination) = destinations.get(&file.package_id) {
            let target = self.config.capacity.target_for(destination).await;
            if self.config.capacity.is_blocked(target).await {
                return Ok(false);
            }
        }
        // A switched-off service must not leave work waiting forever with no reason
        // shown, so the job is blocked instead of skipped.
        if self.kind_disabled(file.kind).await {
            if !blocked_kinds.contains(&file.kind) {
                blocked_kinds.push(file.kind);
                self.block_queued_of_kind(file.kind).await;
            }
            return Ok(false);
        }
        Ok(true)
    }

    /// One link of a mirror group at a time. Enqueueing already picks the member that
    /// runs; this catches the case where somebody started a waiting one by hand, and
    /// stands the loser down rather than fetching the same bytes twice.
    ///
    /// `true` when `file` was stood down (`Skipped`).
    pub(super) async fn mirror_stands_down(
        &self,
        file: &rd_core::DownloadFile,
        groups: &mut HashMap<PackageId, Vec<rd_core::DownloadFile>>,
    ) -> Result<bool> {
        if let std::collections::hash_map::Entry::Vacant(slot) = groups.entry(file.package_id) {
            slot.insert(self.database.downloads_for_package(file.package_id).await?);
        }
        let siblings = groups
            .get(&file.package_id)
            .map(|members| mirrors::siblings(file, members))
            .unwrap_or_default();
        let taken = siblings
            .iter()
            .any(|sibling| mirrors::has_taken_the_turn(sibling.state));
        // Decided by `best_candidate` rather than by which one this loop reached
        // first, so two links added together always resolve the same way round.
        let mut contenders: Vec<&rd_core::DownloadFile> = siblings
            .into_iter()
            .filter(|sibling| mirrors::is_contending(sibling.state))
            .collect();
        contenders.push(file);
        let loses = mirrors::best_candidate(&contenders).is_some_and(|winner| winner.id != file.id);
        if taken || loses {
            self.database
                .transition_download(file.id, DownloadState::Skipped)
                .await?;
            // Read again on the next member of the group, which must see this one
            // standing by rather than contending.
            groups.remove(&file.package_id);
            return Ok(true);
        }
        Ok(false)
    }

    /// The slots `file` starts with: its runner and a slot of its kind, or, for the built-in
    /// worker, its provider's permit. `None` when one of them is not to be had now.
    pub(super) async fn acquire_slots(
        &self,
        file: &rd_core::DownloadFile,
    ) -> Result<Option<Slots>> {
        let external = match file.kind {
            rd_core::DownloadKind::Http => None,
            kind => {
                let Some(runner) = self.runners.get(kind) else {
                    return Ok(None);
                };
                let requested = self.external_parallel_files.load(Ordering::Acquire);
                let Some(permit) = self.runners.try_slot(kind, requested).await else {
                    return Ok(None);
                };
                Some((runner, permit))
            }
        };
        let provider_permit = if external.is_some() {
            None
        } else {
            match self
                .try_provider_slot(file.id, file.account_id, &file.source)
                .await?
            {
                ProviderSlot::Unrestricted => None,
                ProviderSlot::Acquired(permit) => Some(permit),
                ProviderSlot::Busy => return Ok(None),
            }
        };
        Ok(Some(Slots {
            external,
            provider_permit,
        }))
    }

    /// Enters `file` into the active set under its lock, if its host has a connection left
    /// and the global cap admits it.
    pub(super) async fn claim_slot(
        &self,
        file: &rd_core::DownloadFile,
        exempt: bool,
        pooled: bool,
        active_limit: usize,
        host: Option<HostClaim>,
        cancellation: &CancellationToken,
    ) -> Claim {
        let mut active = self.active.lock().await;
        // Asked under the lock `shutdown` collects the tokens under: a pass that was
        // already running when the shutdown began would otherwise add a token nobody
        // cancels and start a job while the WAL is checkpointed (audit 1.9.1, TR-06).
        if self.shutdown.is_cancelled() {
            return Claim::ShuttingDown;
        }
        if active.untouchable(&file.id) {
            return Claim::Refused;
        }
        // Asked under the same lock as the files it counts, so two passes cannot both
        // promise the last connection.
        if let Some(claim) = &host
            && !active.host.has_room(claim, std::time::Instant::now())
        {
            return Claim::HostBusy(claim.host.clone());
        }
        // Exempt kinds (recordings) start regardless of the global cap, and so does
        // another file of a pooled kind that is running already, so keep scanning
        // instead of breaking when the cap is reached.
        if !active.admits(file.kind, exempt, pooled, active_limit) {
            return Claim::Refused;
        }
        active.tokens.insert(file.id, cancellation.clone());
        if exempt {
            active.exempt.insert(file.id);
        } else if pooled {
            active.pooled.insert(file.id, file.kind);
        }
        if let Some(claim) = host {
            active.host.start(file.id, claim);
        }
        Claim::Claimed
    }
}
