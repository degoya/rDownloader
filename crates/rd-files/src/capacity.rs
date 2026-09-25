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
        let state = self.state.read().await;
        state
            .roots
            .iter()
            .filter(|root| destination.starts_with(&root.path))
            .max_by_key(|root| root.path.as_os_str().len())
            .and_then(|root| root.minimum_free_bytes)
            .unwrap_or_else(|| state.settings.storage_minimum_free_bytes.get())
    }

    /// Probes the filesystem behind `destination` and applies the threshold.
    ///
    /// `required` is the number of bytes still to be written; `None` means no runner could
    /// state a size, which falls back to the documented headroom policy.
    pub async fn check(
        &self,
        destination: &Path,
        required: Option<u64>,
    ) -> Result<CapacityVerdict> {
        let (minimum, headroom) = {
            let state = self.state.read().await;
            let minimum = state
                .roots
                .iter()
                .filter(|root| destination.starts_with(&root.path))
                .max_by_key(|root| root.path.as_os_str().len())
                .and_then(|root| root.minimum_free_bytes)
                .unwrap_or_else(|| state.settings.storage_minimum_free_bytes.get());
            (minimum, state.settings.storage_unknown_size_headroom)
        };
        let free = available_space(destination).await?;
        Ok(evaluate_capacity(free, required, minimum, headroom))
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

async fn available_space(path: &Path) -> Result<u64> {
    let path = path.to_path_buf();
    Ok(tokio::task::spawn_blocking(move || fs2::available_space(&path)).await??)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rd_core::{StorageRootId, StorageSettings};

    use super::{CapacityService, CapacityVerdict, RootLimit, StorageTarget, evaluate_capacity};

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
