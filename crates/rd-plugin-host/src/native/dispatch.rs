//! The calls that pick a resolver from the chain and run it: resolve, the free-download and
//! claim questions, the account check, the hoster catalogue, the link check and the
//! concurrency route.
//!
//! Split out of `mod.rs` (PLUG-21); the chain itself and how it is loaded stay there.

use std::sync::{Arc, PoisonError};

use rd_core::{AccountId, Failure, FailureKind, LinkCheckResult, ProxyProfileId, ResolverPin};
use rd_plugin_api::{
    AccountStatus, CheckRequest, ClientIdentity, ResolveRequest, ResolvedDownload, Resolver,
};
use url::Url;

use super::{ResolverService, account_provider, account_required};

impl ResolverService {
    /// Resolves with the explicitly selected provider account, or returns direct HTTP unchanged.
    /// Without an account, a resolver that declares `requires_account: false` may still resolve
    /// the link (free download); everything else falls through to direct HTTP.
    pub async fn resolve(
        &self,
        url: Url,
        account_id: Option<AccountId>,
        proxy_profile_id: Option<ProxyProfileId>,
        pin: Option<&ResolverPin>,
    ) -> Result<Option<ResolvedDownload>, Failure> {
        // Before any installed resolver sees the link: a marker inside it would
        // be expanded with the account's credential (RD-120-66).
        if crate::foreign_address::carries_marker(url.as_str()) {
            return Err(crate::foreign_address::refused());
        }
        let Some(account_id) = account_id else {
            let chain = self.chain();
            let resolver = chain.resolvers.iter().find(|resolver| {
                !resolver.metadata().requires_account
                    && chain.admits(resolver, pin)
                    && resolver.matches(&url)
            });
            let Some(resolver) = resolver.cloned() else {
                // A hoster link with no free path must fail visibly. Falling through to
                // direct HTTP would download the hoster's landing page and store it under
                // the link's name as a completed download.
                if let Some(failure) = account_required(&url) {
                    return Err(failure);
                }
                return Ok(None);
            };
            return resolver
                .resolve(ResolveRequest {
                    url,
                    client: ClientIdentity {
                        account_id: None,
                        proxy_profile_id,
                        tls_revision: 0,
                    },
                })
                .await
                .map(Some);
        };
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self
            .resolver_for_provider(&provider, pin)?
            .filter(|resolver| resolver.matches(&url));
        let Some(resolver) = resolver else {
            // The selected account cannot serve this link (a plain direct URL routed
            // through a multihoster account is the legitimate case); a hoster link still
            // must not degrade into a landing-page download.
            if let Some(failure) = account_required(&url) {
                return Err(failure);
            }
            return Ok(None);
        };
        resolver
            .resolve(ResolveRequest {
                url,
                client: ClientIdentity {
                    account_id: Some(account_id),
                    proxy_profile_id,
                    tls_revision: 0,
                },
            })
            .await
            .map(Some)
    }

    /// The plugin whose resolver can serve this link without an account, if any.
    ///
    /// Two callers need this: the link check, to explain a link it cannot verify but that
    /// will still download, and the scheduler, to serialise a hoster's free downloads.
    #[must_use]
    pub fn free_resolver_plugin(&self, url: &Url) -> Option<rd_core::PluginId> {
        let chain = self.chain();
        chain
            .resolvers
            .iter()
            .find(|resolver| {
                !resolver.metadata().requires_account
                    && chain.selectable(resolver)
                    && resolver.matches(url)
            })
            .map(|resolver| resolver.metadata().plugin_id)
    }

    /// Whether some installed resolver can serve this link without an account.
    #[must_use]
    pub fn has_free_resolver(&self, url: &Url) -> bool {
        self.free_resolver_plugin(url).is_some()
    }

    /// Whether any installed resolver speaks for this address at all -- free or account-bound.
    ///
    /// Broader than [`Self::has_free_resolver`] on purpose, and asked by exactly one caller:
    /// the verdict a crawled address passes before it may become a candidate (RD-110-07).
    /// There the question is not "can this be downloaded right now" but "is this a hoster
    /// link rather than an arbitrary page" -- an address a resolver claims is one the resolver
    /// turns into a file, so nothing is gained by fetching it here to look at its content
    /// type, and a HEAD against a hoster's landing page would answer `text/html` anyway.
    #[must_use]
    pub fn has_resolver(&self, url: &Url) -> bool {
        let chain = self.chain();
        chain
            .resolvers
            .iter()
            .any(|resolver| chain.selectable(resolver) && resolver.matches(url))
    }

    /// Runs the provider resolver's redaction-safe account check.
    pub async fn check_account(&self, account_id: AccountId) -> Result<AccountStatus, Failure> {
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self.provider_resolver(&provider)?;
        resolver.check_account(account_id).await
    }

    /// Hoster catalogue of the account's provider resolver.
    pub async fn hosters(&self, account_id: AccountId) -> Result<Vec<String>, Failure> {
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self.provider_resolver(&provider)?;
        resolver.hosters(account_id).await
    }

    /// Probes links through the account's provider resolver (no download, no domain filter:
    /// multihosters check foreign hosters).
    pub async fn check(
        &self,
        account_id: AccountId,
        urls: Vec<Url>,
    ) -> Result<Vec<LinkCheckResult>, Failure> {
        let (urls, mut unknown) = crate::foreign_address::checkable(urls);
        if urls.is_empty() {
            return Ok(unknown);
        }
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self.provider_resolver(&provider)?;
        let mut checked = resolver
            .check(CheckRequest {
                urls,
                client: ClientIdentity {
                    account_id: Some(account_id),
                    proxy_profile_id: None,
                    tls_revision: 0,
                },
            })
            .await?;
        checked.append(&mut unknown);
        Ok(checked)
    }

    /// Returns the manifest concurrency route selected by an account.
    pub async fn concurrency_route(
        &self,
        account_id: Option<AccountId>,
        pin: Option<&ResolverPin>,
    ) -> Result<Option<(ResolverPin, u32)>, Failure> {
        let Some(account_id) = account_id else {
            return Ok(None);
        };
        let provider = account_provider(&self.database, account_id).await?;
        Ok(self.resolver_for_provider(&provider, pin)?.map(|resolver| {
            let metadata = resolver.metadata();
            (
                ResolverPin {
                    plugin_id: metadata.plugin_id,
                    version: metadata.version.clone(),
                },
                metadata.max_concurrent_downloads,
            )
        }))
    }

    /// The unpinned resolver of an account's provider, or why there is none.
    fn provider_resolver(&self, provider: &str) -> Result<Arc<dyn Resolver>, Failure> {
        self.resolver_for_provider(provider, None)?
            .ok_or_else(|| self.missing_resolver(provider))
    }

    /// No resolver serves `provider`. When its plugin is installed and only waits for the next
    /// start (RD-170-12), that is what the person reads — "no resolver is installed" is untrue
    /// then, and it sent the owner looking for a plugin that was right there.
    fn missing_resolver(&self, provider: &str) -> Failure {
        let waiting = rd_provider_registry::by_slug(provider.trim())
            .and_then(|spec| spec.plugin_id)
            .and_then(|id| id.parse::<rd_core::PluginId>().ok())
            .is_some_and(|id| {
                self.waiting
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains(&id)
            });
        if waiting {
            return Failure::coded(
                FailureKind::Unsupported,
                "plugin.installed_not_running",
                "The plugin for this provider is installed; it runs after the next restart",
            );
        }
        Failure::coded(
            FailureKind::Unsupported,
            "plugin.resolver_missing",
            "No resolver is installed for this provider",
        )
    }

    pub(super) fn resolver_for_provider(
        &self,
        provider: &str,
        pin: Option<&ResolverPin>,
    ) -> Result<Option<Arc<dyn Resolver>>, Failure> {
        let chain = self.chain();
        let resolver = chain
            .resolvers
            .iter()
            .find(|resolver| {
                let provider_matches = resolver
                    .metadata()
                    .provider_slug
                    .eq_ignore_ascii_case(provider.trim());
                provider_matches && chain.admits(resolver, pin)
            })
            .cloned();
        if pin.is_some() && resolver.is_none() {
            return Err(Failure::coded(
                FailureKind::Unsupported,
                "plugin.pinned_version_missing",
                "The plugin version pinned to the job is not installed",
            ));
        }
        Ok(resolver)
    }
}
