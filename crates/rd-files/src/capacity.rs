//! One capacity policy for every runner.
//!
//! Before RD-050-15 each transport carried its own free-space check with its own hard-coded
//! reserve, and only HTTP and Usenet had one at all. This service is the single place that
//! answers "may this transfer start on that filesystem", so torrent, media, gallery and
//! stream jobs are covered by the same rule and the same configurable threshold.
//!
//! It deliberately holds no database handle: the scheduler pushes roots and settings in
//! (like [`rd_core::PostprocessHold`], the runners share the handle), which keeps the crate
//! dependency-light and the policy testable without a database.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use rd_core::{StorageRootId, StorageSettings};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// A filesystem whose free space is tracked and which can be blocked on its own.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "id")]
pub enum StorageTarget {
    Root(StorageRootId),
    /// The service's download directory, used by packages that belong to no configured
    /// root (a direct enqueue without a category).
    Fallback,
}

impl StorageTarget {
    /// The key of the service download directory, which has no root id.
    pub const FALLBACK_KEY: &'static str = "fallback";
    /// What the service download directory is called where a root shows its name.
    pub const FALLBACK_NAME: &'static str = "Downloads";

    /// The target's key: the root id, or [`Self::FALLBACK_KEY`]. The REST path segment of the
    /// manual release and the `target` of a `storage.capacity` event (RD-1270-18).
    #[must_use]
    pub fn key(self) -> String {
        match self {
            Self::Root(id) => id.to_string(),
            Self::Fallback => Self::FALLBACK_KEY.to_owned(),
        }
    }
}

/// One configured storage root with its effective threshold.
#[derive(Clone, Debug)]
pub struct RootLimit {
    pub id: StorageRootId,
    pub path: PathBuf,
    /// `None` inherits the global threshold.
    pub minimum_free_bytes: Option<u64>,
}

/// Why a target cannot take more work. Carries the numbers the UI shows verbatim.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapacityShortfall {
    /// Bytes the transfer needs including the threshold that must remain free.
    pub required_bytes: u64,
    pub free_bytes: u64,
    pub minimum_free_bytes: u64,
    /// `false` when no runner could state a size and the headroom policy was applied.
    pub size_known: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapacityVerdict {
    Ok { free_bytes: u64 },
    Insufficient(CapacityShortfall),
}

impl CapacityVerdict {
    #[must_use]
    pub fn is_ok(self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    #[must_use]
    pub fn shortfall(self) -> Option<CapacityShortfall> {
        match self {
            Self::Ok { .. } => None,
            Self::Insufficient(shortfall) => Some(shortfall),
        }
    }
}

#[derive(Debug, Default)]
struct State {
    settings: StorageSettings,
    roots: Vec<RootLimit>,
    fallback: Option<PathBuf>,
    blocked: HashMap<StorageTarget, CapacityShortfall>,
}

impl State {
    /// The threshold and the unknown-size headroom that apply to a destination: the root's own
    /// threshold, else the global one.
    fn limits(&self, destination: &Path) -> (u64, u32) {
        let minimum = self
            .roots
            .iter()
            .filter(|root| destination.starts_with(&root.path))
            .max_by_key(|root| root.path.as_os_str().len())
            .and_then(|root| root.minimum_free_bytes)
            .unwrap_or_else(|| self.settings.storage_minimum_free_bytes.get());
        (minimum, self.settings.storage_unknown_size_headroom)
    }
}

/// Cloneable handle shared by the scheduler, the runners and the REST layer.
#[derive(Clone, Debug, Default)]
pub struct CapacityService {
    state: Arc<RwLock<State>>,
}

impl CapacityService {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the configured roots, the fallback directory and the global thresholds.
    pub async fn apply(
        &self,
        settings: StorageSettings,
        roots: Vec<RootLimit>,
        fallback: Option<PathBuf>,
    ) {
        let mut state = self.state.write().await;
        state.settings = settings;
        state.roots = roots;
        state.fallback = fallback;
        // A root that disappeared cannot stay blocked, or its block would be unclearable.
        let known = state
            .roots
            .iter()
            .map(|root| StorageTarget::Root(root.id))
            .chain(std::iter::once(StorageTarget::Fallback))
            .collect::<Vec<_>>();
        state.blocked.retain(|target, _| known.contains(target));
    }

    pub async fn settings(&self) -> StorageSettings {
        self.state.read().await.settings
    }

    /// The target a destination belongs to; the longest matching root wins, so a root
    /// nested inside another one is attributed to the nested one.
    pub async fn target_for(&self, destination: &Path) -> StorageTarget {
        let state = self.state.read().await;
        state
            .roots
            .iter()
            .filter(|root| destination.starts_with(&root.path))
            .max_by_key(|root| root.path.as_os_str().len())
            .map_or(StorageTarget::Fallback, |root| StorageTarget::Root(root.id))
    }

    /// Every configured target with the path its free space is probed on.
    pub async fn targets(&self) -> Vec<(StorageTarget, PathBuf)> {
        let state = self.state.read().await;
        let mut targets = state
            .roots
            .iter()
            .map(|root| (StorageTarget::Root(root.id), root.path.clone()))
            .collect::<Vec<_>>();
        if let Some(fallback) = state.fallback.clone() {
            targets.push((StorageTarget::Fallback, fallback));
        }
        targets
    }

    /// Threshold that applies to a destination: the root's own value, else the global one.
    pub async fn minimum_free_bytes(&self, destination: &Path) -> u64 {
        self.state.read().await.limits(destination).0
    }

    /// Probes the filesystem behind `destination` and applies the threshold.
    ///
    /// `required` is the number of bytes still to be written; `None` means no runner could
    /// state a size, which falls back to the documented headroom policy. A destination not
    /// created yet is probed on its nearest existing ancestor ([`space_for_destination`]).
    pub async fn check(
        &self,
        destination: &Path,
        required: Option<u64>,
    ) -> Result<CapacityVerdict> {
        let free = space_for_destination(destination).await?;
        Ok(self.evaluate(destination, free, required).await)
    }

    /// Applies the start check to a free-space figure already probed: the rule
    /// [`Self::check`] applies, for a caller that probes once and asks for several sizes. The
    /// release of a blocked target asks it for each download waiting there (RD-1270-07).
    pub async fn evaluate(
        &self,
        destination: &Path,
        free_bytes: u64,
        required: Option<u64>,
    ) -> CapacityVerdict {
        let (minimum, headroom) = self.state.read().await.limits(destination);
        evaluate_capacity(free_bytes, required, minimum, headroom)
    }

    /// Free space on a target's filesystem, for the periodic growth check.
    pub async fn probe(&self, path: &Path) -> Result<u64> {
        available_space(path).await
    }

    #[must_use]
    pub async fn is_blocked(&self, target: StorageTarget) -> bool {
        self.state.read().await.blocked.contains_key(&target)
    }

    pub async fn shortfall(&self, target: StorageTarget) -> Option<CapacityShortfall> {
        self.state.read().await.blocked.get(&target).copied()
    }

    pub async fn blocked(&self) -> Vec<(StorageTarget, CapacityShortfall)> {
        self.state
            .read()
            .await
            .blocked
            .iter()
            .map(|(target, shortfall)| (*target, *shortfall))
            .collect()
    }

    /// Blocks or releases a target. Returns `true` when the state actually changed, so the
    /// caller only persists and broadcasts on a real transition.
    pub async fn set_blocked(
        &self,
        target: StorageTarget,
        shortfall: Option<CapacityShortfall>,
    ) -> bool {
        let mut state = self.state.write().await;
        match shortfall {
            Some(shortfall) => state.blocked.insert(target, shortfall).is_none(),
            None => state.blocked.remove(&target).is_some(),
        }
    }

    /// Restores blocks persisted before a restart, so a root does not silently resume when
    /// automatic resume is switched off.
    pub async fn restore(&self, blocked: Vec<(StorageTarget, CapacityShortfall)>) {
        let mut state = self.state.write().await;
        state.blocked = blocked.into_iter().collect();
    }
}

/// Applies the threshold to one probe result.
///
/// A known size must fit *and* leave the threshold free; an unknown size may start while a
/// multiple of the threshold is free, because nothing can predict how far it will grow.
#[must_use]
pub(crate) fn evaluate_capacity(
    free_bytes: u64,
    required: Option<u64>,
    minimum_free_bytes: u64,
    unknown_size_headroom: u32,
) -> CapacityVerdict {
    let size_known = required.is_some();
    let required_bytes = match required {
        Some(size) => size.saturating_add(minimum_free_bytes),
        None => minimum_free_bytes.saturating_mul(u64::from(unknown_size_headroom.max(1))),
    };
    if free_bytes >= required_bytes {
        CapacityVerdict::Ok { free_bytes }
    } else {
        CapacityVerdict::Insufficient(CapacityShortfall {
            required_bytes,
            free_bytes,
            minimum_free_bytes,
            size_known,
        })
    }
}

/// Free bytes on the filesystem behind `path`, without any threshold applied.
///
/// Public for the callers the scheduler's check does not answer: direct unpack asks whether a
/// package's filesystem can hold the unpacked set beside the volumes still arriving
/// (RD-1100-07), and the Usenet assembly whether the gaps of missing articles fit (RD-1101-16).
pub async fn available_space(path: &Path) -> Result<u64> {
    let path = path.to_path_buf();
    Ok(tokio::task::spawn_blocking(move || fs4::available_space(&path)).await??)
}

/// Free bytes on the filesystem `destination` will be created on: its own, or its nearest
/// existing ancestor's while it does not exist yet (RD-1270-08).
///
/// A package's folder is created by its runner, after the start check: probed on the folder
/// alone, a fresh external package failed the probe, and the check let every one through
/// unchecked. An ancestor that cannot be read (a share that is down) or a path with no
/// existing part stays an error, which the caller treats as "not probeable".
async fn space_for_destination(destination: &Path) -> Result<u64> {
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<u64> {
        for candidate in destination.ancestors() {
            if candidate.as_os_str().is_empty() {
                break;
            }
            if candidate.try_exists()? {
                return Ok(fs4::available_space(candidate)?);
            }
        }
        anyhow::bail!("no part of {} exists", destination.display())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rd_core::{StorageRootId, StorageSettings};

    use super::{
        CapacityService, CapacityVerdict, RootLimit, StorageTarget, evaluate_capacity,
        space_for_destination,
    };

    /// RD-1270-08: a destination not created yet is checked on the disk it will land on.
    #[tokio::test]
    async fn a_missing_destination_is_probed_on_its_nearest_existing_ancestor() {
        let directory = tempfile::tempdir().expect("temp");
        let missing = directory.path().join("package").join("nested");
        // On Unix the folder alone cannot be probed while it does not exist — the defect this
        // fixes. Windows answers for any path on a mounted volume, so there the probe never failed.
        #[cfg(unix)]
        assert!(super::available_space(&missing).await.is_err());
        assert!(space_for_destination(&missing).await.is_ok());
        // More than any disk has free: the start check now refuses it instead of skipping it.
        let verdict = CapacityService::new()
            .check(&missing, Some(1_u64 << 60))
            .await
            .expect("probed on the ancestor");
        assert!(verdict.shortfall().is_some());
        // Nothing of a relative path exists: still "not probeable".
        assert!(
            space_for_destination(&PathBuf::from("rd-files-missing-1270-08/package"))
                .await
                .is_err()
        );
    }

    #[test]
    fn a_known_size_must_fit_and_leave_the_threshold_free() {
        assert!(evaluate_capacity(1_500, Some(1_000), 400, 4).is_ok());
        let shortfall = evaluate_capacity(1_200, Some(1_000), 400, 4)
            .shortfall()
            .expect("blocked");
        assert_eq!(shortfall.required_bytes, 1_400);
        assert!(shortfall.size_known);
    }

    #[test]
    fn an_unknown_size_needs_the_headroom_multiple() {
        assert!(evaluate_capacity(1_600, None, 400, 4).is_ok());
        let shortfall = evaluate_capacity(1_500, None, 400, 4)
            .shortfall()
            .expect("blocked");
        assert_eq!(shortfall.required_bytes, 1_600);
        assert!(!shortfall.size_known);
    }

    #[tokio::test]
    async fn the_longest_matching_root_owns_a_destination() {
        let service = CapacityService::new();
        let outer = StorageRootId::new();
        let inner = StorageRootId::new();
        service
            .apply(
                StorageSettings::default(),
                vec![
                    RootLimit {
                        id: outer,
                        path: PathBuf::from("/data"),
                        minimum_free_bytes: Some(100),
                    },
                    RootLimit {
                        id: inner,
                        path: PathBuf::from("/data/fast"),
                        minimum_free_bytes: Some(900),
                    },
                ],
                Some(PathBuf::from("/downloads")),
            )
            .await;
        assert_eq!(
            service
                .target_for(&PathBuf::from("/data/fast/movies"))
                .await,
            StorageTarget::Root(inner)
        );
        assert_eq!(
            service
                .target_for(&PathBuf::from("/data/slow/movies"))
                .await,
            StorageTarget::Root(outer)
        );
        assert_eq!(
            service.target_for(&PathBuf::from("/elsewhere")).await,
            StorageTarget::Fallback
        );
        assert_eq!(
            service
                .minimum_free_bytes(&PathBuf::from("/data/fast/movies"))
                .await,
            900
        );
    }

    #[tokio::test]
    async fn only_a_real_transition_reports_a_change() {
        let service = CapacityService::new();
        let target = StorageTarget::Fallback;
        let shortfall = match evaluate_capacity(0, Some(10), 5, 4) {
            CapacityVerdict::Insufficient(shortfall) => shortfall,
            CapacityVerdict::Ok { .. } => unreachable!("blocked"),
        };
        assert!(service.set_blocked(target, Some(shortfall)).await);
        assert!(!service.set_blocked(target, Some(shortfall)).await);
        assert!(service.is_blocked(target).await);
        assert!(service.set_blocked(target, None).await);
        assert!(!service.set_blocked(target, None).await);
    }

    #[tokio::test]
    async fn evaluate_applies_the_start_check_to_a_probed_figure() {
        let service = CapacityService::new();
        let root = StorageRootId::new();
        service
            .apply(
                StorageSettings::default(),
                vec![RootLimit {
                    id: root,
                    path: PathBuf::from("/data"),
                    minimum_free_bytes: Some(400),
                }],
                None,
            )
            .await;
        let destination = PathBuf::from("/data/movies");
        let headroom = StorageSettings::default().storage_unknown_size_headroom;
        for (free, required) in [(1_399, Some(1_000)), (1_400, Some(1_000)), (1_599, None)] {
            assert_eq!(
                service.evaluate(&destination, free, required).await,
                evaluate_capacity(free, required, 400, headroom)
            );
        }
        assert!(
            service
                .evaluate(&destination, 400 * u64::from(headroom), None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_removed_root_does_not_stay_blocked() {
        let service = CapacityService::new();
        let root = StorageRootId::new();
        service
            .apply(
                StorageSettings::default(),
                vec![RootLimit {
                    id: root,
                    path: PathBuf::from("/data"),
                    minimum_free_bytes: None,
                }],
                None,
            )
            .await;
        let shortfall = evaluate_capacity(0, Some(10), 5, 4)
            .shortfall()
            .expect("blocked");
        service
            .set_blocked(StorageTarget::Root(root), Some(shortfall))
            .await;
        service
            .apply(StorageSettings::default(), Vec::new(), None)
            .await;
        assert!(!service.is_blocked(StorageTarget::Root(root)).await);
    }
}
