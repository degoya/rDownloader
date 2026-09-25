//! Native `Resolver` implementation.
//!
//! Nothing but metadata and conversions: the protocol logic lives in `crate::resolver` and is
//! the same code the WebAssembly component runs.

use std::sync::Arc;

use async_trait::async_trait;
use plugin_common::native::{
    NativeHost, to_check_input, to_native_account, to_native_checks, to_native_failure,
    to_native_resolved, to_resolve_input,
};
use rd_core::{AccountId, Failure, LinkCheckResult};
use rd_plugin_api::{
    AccountStatus, CheckRequest, ClientIdentity, ResolveRequest, ResolvedDownload, Resolver,
    ResolverHost, ResolverMetadata,
};
use url::Url;

use crate::resolver;

/// Provider implementation that runs against a native or Component host adapter.
pub struct NitroflareResolver {
    host: Arc<dyn ResolverHost>,
    metadata: ResolverMetadata,
}

impl NitroflareResolver {
    #[must_use]
    pub fn new(host: Arc<dyn ResolverHost>) -> Self {
        Self {
            host,
            metadata: rd_plugin_api::metadata_from_manifest(crate::MANIFEST),
        }
    }

    fn for_account(&self, account_id: AccountId) -> NativeHost {
        NativeHost::for_account(Arc::clone(&self.host), account_id)
    }

    fn for_client(&self, client: ClientIdentity) -> NativeHost {
        NativeHost::new(Arc::clone(&self.host), client)
    }
}

#[async_trait]
impl Resolver for NitroflareResolver {
    fn metadata(&self) -> &ResolverMetadata {
        &self.metadata
    }

    fn matches(&self, url: &Url) -> bool {
        resolver::matches(url.as_str())
    }

    async fn check_account(&self, account_id: AccountId) -> Result<AccountStatus, Failure> {
        let host = self.for_account(account_id);
        resolver::check_account(&host, &account_id.to_string())
            .await
            .map(to_native_account)
            .map_err(to_native_failure)
    }

    async fn resolve(&self, request: ResolveRequest) -> Result<ResolvedDownload, Failure> {
        let input = to_resolve_input(&request);
        let host = self.for_client(request.client.clone());
        let resolved = resolver::resolve(&host, &input)
            .await
            .map_err(to_native_failure)?;
        to_native_resolved(resolved, request.client)
    }

    async fn check(&self, request: CheckRequest) -> Result<Vec<LinkCheckResult>, Failure> {
        let input = to_check_input(&request);
        let host = self.for_client(request.client);
        resolver::check(&host, &input)
            .await
            .map(to_native_checks)
            .map_err(to_native_failure)
    }

    async fn hosters(&self, account_id: AccountId) -> Result<Vec<String>, Failure> {
        let host = self.for_account(account_id);
        resolver::hosters(&host, &account_id.to_string())
            .await
            .map_err(to_native_failure)
    }
}

#[cfg(test)]
#[path = "native/tests.rs"]
mod tests;
