//! `RemoteJobDriver`, the seam between the sweep and one plugin, and its implementation over
//! the host wrapper.
//!
//! Split out of `remote_job.rs` (PLUG-21).

use anyhow::Result;
use async_trait::async_trait;
use rd_core::AccountId;
use rd_plugin_host::extension::{
    CacheAnswer, CacheKind, CacheQuery, RemoteJobHandle, RemoteJobPlugin, RemoteJobProgress,
    RemoteJobRefusal, RemoteJobSource,
};

/// What the sweep needs of one plugin.
///
/// Mirrors the host wrapper call for call. A trait rather than the wrapper itself so the
/// order of writes in the sweep -- the whole idempotency argument -- can be exercised against
/// a mock provider that records what it was asked, without a toolchain in the loop.
#[doc(hidden)]
#[async_trait]
pub trait RemoteJobDriver: Send + Sync {
    async fn claims(&self, source: &RemoteJobSource) -> Result<bool>;
    async fn identify(&self, source: &RemoteJobSource) -> Result<Result<String, RemoteJobRefusal>>;
    async fn submit(
        &self,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>>;
    /// [`Self::submit`], with the name the source was added under. The default forgets the
    /// name, which is what a driver without a `job-context` does.
    async fn submit_named(
        &self,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
        _source_name: Option<&str>,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        self.submit(account, source, content_key).await
    }
    async fn adopt(
        &self,
        account: AccountId,
        content_key: &str,
    ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>>;
    async fn poll(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>>;
    async fn choose(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
        chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>>;
    async fn discard(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>>;

    /// The kinds of source the provider's cache can be asked about (RD-130-11). The default
    /// is none, which is what every provider without a cache query answers.
    async fn cache_kinds(&self) -> Result<Vec<CacheKind>> {
        Ok(Vec::new())
    }

    /// Whether the provider holds each source ready. The default answers `Unknown` for every
    /// query without asking anybody.
    async fn check_cached(
        &self,
        _account: AccountId,
        queries: &[CacheQuery],
    ) -> Result<Result<Vec<CacheAnswer>, RemoteJobRefusal>> {
        Ok(Ok(vec![CacheAnswer::unknown(); queries.len()]))
    }
}

#[async_trait]
impl RemoteJobDriver for RemoteJobPlugin {
    async fn claims(&self, source: &RemoteJobSource) -> Result<bool> {
        Self::claims(self, source).await
    }

    async fn identify(&self, source: &RemoteJobSource) -> Result<Result<String, RemoteJobRefusal>> {
        Self::identify(self, source).await
    }

    async fn submit(
        &self,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        Self::submit(self, account, source, content_key).await
    }

    async fn submit_named(
        &self,
        account: AccountId,
        source: &RemoteJobSource,
        content_key: &str,
        source_name: Option<&str>,
    ) -> Result<Result<RemoteJobHandle, RemoteJobRefusal>> {
        Self::submit_named(self, account, source, content_key, source_name).await
    }

    async fn adopt(
        &self,
        account: AccountId,
        content_key: &str,
    ) -> Result<Result<Option<RemoteJobHandle>, RemoteJobRefusal>> {
        Self::adopt(self, account, content_key).await
    }

    async fn poll(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<RemoteJobProgress, RemoteJobRefusal>> {
        Self::poll(self, account, handle).await
    }

    async fn choose(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
        chosen: &[u32],
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Self::choose(self, account, handle, chosen).await
    }

    async fn discard(
        &self,
        account: AccountId,
        handle: &RemoteJobHandle,
    ) -> Result<Result<(), RemoteJobRefusal>> {
        Self::discard(self, account, handle).await
    }

    async fn cache_kinds(&self) -> Result<Vec<CacheKind>> {
        Self::cache_kinds(self).await
    }

    async fn check_cached(
        &self,
        account: AccountId,
        queries: &[CacheQuery],
    ) -> Result<Result<Vec<CacheAnswer>, RemoteJobRefusal>> {
        Self::check_cached(self, account, queries).await
    }
}
