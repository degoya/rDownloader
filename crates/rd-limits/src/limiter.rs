//! Hierarchical byte-per-second limiters.
//!
//! A transfer acquires its bytes from every bucket that applies to it — the global one plus
//! the protocol, host, account and category buckets of its scope. Because each bucket in
//! the chain has to release the bytes, the strictest applicable limit wins without anyone
//! having to compute a minimum, and [`LimiterRegistry::binding_limit`] can still name which
//! one it was.

use std::{
    collections::HashMap,
    num::NonZeroU32,
    sync::{Arc, RwLock},
};

use anyhow::{Context, Result};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};

use crate::scope::{LimitScope, LimitSource, TransferScope};

#[derive(Clone, Debug)]
struct ActiveLimit {
    limiter: Arc<DefaultDirectRateLimiter>,
    capacity: u32,
}

/// A cloneable limiter whose quota can be replaced while transfers are active.
#[derive(Clone, Debug, Default)]
pub struct BandwidthLimiter {
    inner: Arc<RwLock<Option<ActiveLimit>>>,
}

impl BandwidthLimiter {
    /// Creates a limiter. `None` or zero means unlimited.
    #[must_use]
    pub fn new(bytes_per_second: Option<u64>) -> Self {
        let limiter = Self::default();
        limiter.set_limit(bytes_per_second);
        limiter
    }

    /// Replaces the shared quota. `None` or zero switches to unlimited mode.
    pub fn set_limit(&self, bytes_per_second: Option<u64>) {
        let next = bytes_per_second
            .filter(|value| *value > 0)
            .and_then(|rate| {
                let capacity = u32::try_from(rate).unwrap_or(u32::MAX);
                let non_zero = NonZeroU32::new(capacity)?;
                let quota = Quota::per_second(non_zero).allow_burst(non_zero);
                Some(ActiveLimit {
                    limiter: Arc::new(RateLimiter::direct(quota)),
                    capacity,
                })
            });
        *self
            .inner
            .write()
            .unwrap_or_else(|error| error.into_inner()) = next;
    }

    /// Returns the effective byte-per-second limit.
    #[must_use]
    pub fn limit(&self) -> Option<u64> {
        self.inner
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|limit| u64::from(limit.capacity))
    }

    fn active_limit(&self) -> Option<ActiveLimit> {
        self.inner
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Waits until the requested number of bytes may be consumed.
    pub async fn acquire(&self, bytes: usize) -> Result<()> {
        let Some(limit) = self.active_limit() else {
            return Ok(());
        };
        let mut remaining = bytes as u64;
        while remaining > 0 {
            let batch = remaining.min(u64::from(limit.capacity)) as u32;
            let batch = NonZeroU32::new(batch).context("zero bandwidth batch")?;
            limit
                .limiter
                .until_n_ready(batch)
                .await
                .context("bandwidth quota capacity")?;
            remaining -= u64::from(batch.get());
        }
        Ok(())
    }
}

/// The limit that actually constrains a transfer, and where it came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BindingLimit {
    pub bytes_per_second: u64,
    pub source: LimitSource,
}

#[derive(Debug, Default)]
struct RegistryState {
    /// Set by hand and independent of the schedule, so a profile switch never clears it.
    manual: Option<BandwidthLimiter>,
    global: Option<BandwidthLimiter>,
    scoped: HashMap<LimitScope, BandwidthLimiter>,
}

/// The global limiter plus every configured scoped one.
#[derive(Clone, Debug, Default)]
pub struct LimiterRegistry {
    state: Arc<RwLock<RegistryState>>,
}

impl LimiterRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the whole limit set — the profile switch.
    ///
    /// Buckets that survive the switch are updated in place, so a transfer already waiting
    /// on one continues under the new quota instead of restarting its wait.
    pub fn apply(&self, global: Option<u64>, scoped: &[(LimitScope, u64)]) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner());
        match (&state.global, global) {
            (Some(limiter), rate) => limiter.set_limit(rate),
            (None, rate) => state.global = Some(BandwidthLimiter::new(rate)),
        }
        state
            .scoped
            .retain(|scope, _| scoped.iter().any(|(other, _)| other == scope));
        for (scope, rate) in scoped {
            match state.scoped.get(scope) {
                Some(limiter) => limiter.set_limit(Some(*rate)),
                None => {
                    state
                        .scoped
                        .insert(scope.clone(), BandwidthLimiter::new(Some(*rate)));
                }
            }
        }
    }

    /// Replaces the hand-set global limit, which survives every profile switch.
    pub fn set_manual_limit(&self, bytes_per_second: Option<u64>) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|error| error.into_inner());
        match &state.manual {
            Some(limiter) => limiter.set_limit(bytes_per_second),
            None => state.manual = Some(BandwidthLimiter::new(bytes_per_second)),
        }
    }

    /// The limiters a transfer has to pass, from the broadest to the narrowest.
    fn chain(&self, scope: &TransferScope) -> Vec<(LimitSource, BandwidthLimiter)> {
        let state = self.state.read().unwrap_or_else(|error| error.into_inner());
        let mut chain = Vec::new();
        if let Some(manual) = state.manual.clone() {
            chain.push((LimitSource::Manual, manual));
        }
        if let Some(global) = state.global.clone() {
            chain.push((LimitSource::Global, global));
        }
        for (source, key) in scope.keys() {
            if let Some(limiter) = state.scoped.get(&key) {
                chain.push((source, limiter.clone()));
            }
        }
        chain
    }

    /// Waits until every applicable bucket has released `bytes`.
    pub async fn acquire(&self, scope: &TransferScope, bytes: usize) -> Result<()> {
        for (_, limiter) in self.chain(scope) {
            limiter.acquire(bytes).await?;
        }
        Ok(())
    }

    /// The strictest limit applying to a scope, for the "why is this slow" display.
    #[must_use]
    pub fn binding_limit(&self, scope: &TransferScope) -> Option<BindingLimit> {
        self.chain(scope)
            .into_iter()
            .filter_map(|(source, limiter)| {
                limiter.limit().map(|bytes_per_second| BindingLimit {
                    bytes_per_second,
                    source,
                })
            })
            .min_by_key(|limit| limit.bytes_per_second)
    }

    /// A handle that already knows its scope, handed to the transports.
    #[must_use]
    pub fn scoped(&self, scope: TransferScope) -> ScopedLimiter {
        ScopedLimiter {
            registry: self.clone(),
            scope,
        }
    }
}

/// A limiter bound to one transfer's scope; what the transports actually hold.
#[derive(Clone)]
pub struct ScopedLimiter {
    registry: LimiterRegistry,
    scope: TransferScope,
}

impl ScopedLimiter {
    /// A limiter that never waits, for tests and for transports outside the queue.
    #[must_use]
    pub fn unlimited() -> Self {
        Self {
            registry: LimiterRegistry::new(),
            scope: TransferScope::default(),
        }
    }

    pub async fn acquire(&self, bytes: usize) -> Result<()> {
        self.registry.acquire(&self.scope, bytes).await
    }

    #[must_use]
    pub fn binding_limit(&self) -> Option<BindingLimit> {
        self.registry.binding_limit(&self.scope)
    }
}

#[cfg(test)]
mod tests {
    use rd_core::DownloadKind;

    use super::{BandwidthLimiter, LimiterRegistry};
    use crate::scope::{LimitScope, LimitSource, TransferScope};

    #[test]
    fn quota_can_be_reconfigured_through_a_clone() {
        let limiter = BandwidthLimiter::new(None);
        let worker = limiter.clone();
        assert_eq!(worker.limit(), None);

        limiter.set_limit(Some(1024));
        assert_eq!(worker.limit(), Some(1024));

        worker.set_limit(Some(0));
        assert_eq!(limiter.limit(), None);
    }

    #[test]
    fn the_strictest_applicable_limit_is_reported_with_its_scope() {
        let registry = LimiterRegistry::new();
        let scope = TransferScope {
            kind: Some(DownloadKind::Http),
            host: Some("example.com".to_owned()),
            ..TransferScope::default()
        };
        registry.apply(
            Some(1_000_000),
            &[
                (LimitScope::Protocol(DownloadKind::Http), 500_000),
                (LimitScope::Host("example.com".to_owned()), 250_000),
            ],
        );
        let binding = registry.binding_limit(&scope).expect("a limit applies");
        assert_eq!(binding.bytes_per_second, 250_000);
        assert_eq!(binding.source, LimitSource::Host);

        // A scope the narrow bucket does not cover falls back to the next strictest.
        let other = TransferScope {
            kind: Some(DownloadKind::Http),
            host: Some("other.example".to_owned()),
            ..TransferScope::default()
        };
        let binding = registry.binding_limit(&other).expect("a limit applies");
        assert_eq!(binding.bytes_per_second, 500_000);
        assert_eq!(binding.source, LimitSource::Protocol);
    }

    #[test]
    fn a_switch_removes_buckets_the_new_profile_does_not_define() {
        let registry = LimiterRegistry::new();
        let scope = TransferScope {
            kind: Some(DownloadKind::Usenet),
            ..TransferScope::default()
        };
        registry.apply(
            None,
            &[(LimitScope::Protocol(DownloadKind::Usenet), 100_000)],
        );
        assert!(registry.binding_limit(&scope).is_some());
        registry.apply(None, &[]);
        assert!(registry.binding_limit(&scope).is_none());
    }

    #[tokio::test]
    async fn an_unlimited_registry_never_waits() {
        let registry = LimiterRegistry::new();
        registry.apply(None, &[]);
        registry
            .scoped(TransferScope::default())
            .acquire(8 * 1024 * 1024)
            .await
            .expect("no wait");
    }
}

#[cfg(test)]
mod manual_tests {
    use super::LimiterRegistry;
    use crate::scope::{LimitSource, TransferScope};

    #[test]
    fn the_hand_set_limit_survives_a_profile_switch_and_can_be_the_binding_one() {
        let registry = LimiterRegistry::new();
        let scope = TransferScope::default();
        registry.set_manual_limit(Some(100_000));
        registry.apply(Some(400_000), &[]);
        let binding = registry.binding_limit(&scope).expect("a limit applies");
        assert_eq!(binding.source, LimitSource::Manual);
        assert_eq!(binding.bytes_per_second, 100_000);

        // Switching to a stricter profile makes the profile the binding one; the manual
        // limit is still there.
        registry.apply(Some(50_000), &[]);
        let binding = registry.binding_limit(&scope).expect("a limit applies");
        assert_eq!(binding.source, LimitSource::Global);
        registry.apply(None, &[]);
        assert_eq!(
            registry.binding_limit(&scope).map(|limit| limit.source),
            Some(LimitSource::Manual)
        );
    }
}
