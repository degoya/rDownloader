//! Which tool versions a running job is still executing.
//!
//! Activation has to take effect immediately — the next job should get the new version — but
//! a job already running has an open file handle on a binary and a half-finished download
//! that assumes its behaviour. Deleting that version underneath it is the failure this exists
//! to prevent.
//!
//! A lease is a reference count, in memory, keyed by tool and version. A runner takes one
//! when it resolves the binary and drops it when the process is done. Activation never waits
//! for a lease; only *removal* does, and it refuses rather than blocks. The count is
//! deliberately not persisted: a process that is no longer running holds no lease, and a
//! restart is exactly the moment when that becomes true again.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Reference counts for tool versions currently in use.
#[derive(Clone, Debug, Default)]
pub struct LeaseRegistry {
    counts: Arc<Mutex<HashMap<(String, String), usize>>>,
}

impl LeaseRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks `name` at `version` as in use until the returned lease is dropped.
    #[must_use]
    pub fn acquire(&self, name: &str, version: &str) -> rd_core::ToolLease {
        let key = (name.to_owned(), version.to_owned());
        if let Ok(mut counts) = self.counts.lock() {
            *counts.entry(key.clone()).or_insert(0) += 1;
        }
        rd_core::ToolLease::new(Arc::new(LeaseGuard {
            registry: self.clone(),
            key,
        }))
    }

    /// Whether anything currently holds a lease on this version.
    #[must_use]
    pub fn is_leased(&self, name: &str, version: &str) -> bool {
        self.counts.lock().is_ok_and(|counts| {
            counts
                .get(&(name.to_owned(), version.to_owned()))
                .is_some_and(|count| *count > 0)
        })
    }

    fn release(&self, key: &(String, String)) {
        let Ok(mut counts) = self.counts.lock() else {
            return;
        };
        if let Some(count) = counts.get_mut(key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(key);
            }
        }
    }
}

/// The value a [`rd_core::ToolLease`] wraps. Its only job is its `Drop`.
#[derive(Debug)]
struct LeaseGuard {
    registry: LeaseRegistry,
    key: (String, String),
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.registry.release(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lease_is_held_until_the_last_holder_drops_it() {
        let registry = LeaseRegistry::new();
        assert!(!registry.is_leased("yt-dlp", "2024.09.07"));
        let first = registry.acquire("yt-dlp", "2024.09.07");
        let second = registry.acquire("yt-dlp", "2024.09.07");
        assert!(registry.is_leased("yt-dlp", "2024.09.07"));
        drop(first);
        assert!(registry.is_leased("yt-dlp", "2024.09.07"));
        drop(second);
        assert!(!registry.is_leased("yt-dlp", "2024.09.07"));
    }

    /// A lease on one version says nothing about another, or an activation could never
    /// retire anything.
    #[test]
    fn a_lease_covers_exactly_one_version() {
        let registry = LeaseRegistry::new();
        let _held = registry.acquire("yt-dlp", "2024.01.01");
        assert!(registry.is_leased("yt-dlp", "2024.01.01"));
        assert!(!registry.is_leased("yt-dlp", "2024.09.07"));
        assert!(!registry.is_leased("gallery-dl", "2024.01.01"));
    }

    /// Cloning the handle a runner holds must not release the version early.
    #[test]
    fn a_cloned_lease_keeps_the_version_held() {
        let registry = LeaseRegistry::new();
        let lease = registry.acquire("ffmpeg", "7.1");
        let copy = lease.clone();
        drop(lease);
        assert!(registry.is_leased("ffmpeg", "7.1"));
        drop(copy);
        assert!(!registry.is_leased("ffmpeg", "7.1"));
    }
}
