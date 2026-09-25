//! Folder crawlers (RD-104-03): one address in, the files behind it out.
//!
//! The wrapper is deliberately thin, and it is thin in one direction: everything it does
//! beyond calling the guest is *refusing* something the guest said. A crawler follows
//! addresses a stranger controls, so its answer is treated as a proposal from an untrusted
//! party — bounded in number, checked for shape, and stripped of anything that looks like a
//! path rather than a name.

use std::sync::Arc;

use anyhow::Result;
use rd_core::AccountId;
use rd_plugin_api::ResolverHost;

use super::{ExtensionRuntime, bindings::crawler};
use crate::{PluginManifest, runtime::PluginStoreState};

/// Most links one crawl may hand back.
///
/// The plugin has its own, lower limits — a walk that bounds its own depth and breadth is the
/// only way to keep a crawl from making a thousand requests. This one exists because the host
/// cannot assume the plugin kept its word: a crawler that answers with a hundred thousand
/// links is trimmed here rather than obeyed.
pub const MAX_CRAWLED_LINKS: usize = 1_000;

/// A compiled crawler, pinned to one installed manifest version.
pub struct FolderCrawler {
    runtime: ExtensionRuntime,
    pre: crawler::CrawlerPluginPre<PluginStoreState>,
}

/// One file a crawler found, as the guest described it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrawledLink {
    pub url: String,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    pub package_hint: Option<String>,
    /// What the source said about this link being one of several copies of the same file
    /// (RD-110-18).
    ///
    /// Always `None` for a plugin crawler: the `crawled-link` record in the WIT contract has
    /// no field for it, and widening a record changes the ABI of every component already
    /// signed. It is filled by the *rule* side of the crawler selection, which is where a
    /// release page is recognised in the first place (`rd_plugin_ext::siterules`).
    pub mirror_hint: Option<rd_core::MirrorHint>,
}

/// Why a crawl produced nothing, in the shape the interface can translate.
///
/// A code rather than a message, because this is exactly the case the job exists to fix: an
/// empty or unreachable folder has to say so in the language the person reads, and a plugin
/// that could only return English prose would leave the interface guessing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrawlRefusal {
    /// Stable translation code, e.g. `premiumize_crawler.folder_empty`.
    pub code: Option<String>,
    /// English, redaction-safe text; the fallback when no catalogue carries the code.
    pub message: String,
    /// The plugin claimed the address and then found it was not its after all.
    ///
    /// `unsupported` is the one refusal that says nothing about the folder — it says the
    /// plugin was wrong to claim it. A crawler that recognises a share by the shape of its
    /// path rather than by its host cannot avoid being wrong sometimes, and before RD-107-05
    /// being wrong once ended the link: the selection took the first claimer and returned
    /// its answer, refusal included. This is the flag the selection falls back on.
    pub not_mine: bool,
}

impl FolderCrawler {
    /// Compiles a verified package and links only what its manifest grants.
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: crawler::CrawlerPluginPre::new(pre)?,
        })
    }

    /// The manifest this crawler was built from.
    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// Whether the plugin claims this address.
    ///
    /// Asked before every crawl and reaching nothing: the guest answers from the address
    /// alone, so a link that belongs to somebody else is never fetched on its behalf.
    pub async fn claims(&self, url: &str) -> Result<bool> {
        if crate::foreign_address::carries_marker(url) {
            return Ok(false);
        }
        let mut store = self.runtime.store(None)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        Ok(instance
            .rdownloader_plugin_crawler()
            .call_claims_url(&mut store, url)
            .await?)
    }

    /// Enumerates what lies behind an address, for one account.
    ///
    /// `account` is the account whose credential the plugin's requests may expand; without
    /// one it reaches the provider unauthenticated, which is right for a public share and
    /// refused by the provider for anything else.
    pub async fn crawl(
        &self,
        url: &str,
        account: Option<AccountId>,
    ) -> Result<Result<Vec<CrawledLink>, CrawlRefusal>> {
        if crate::foreign_address::carries_marker(url) {
            return Err(crate::foreign_address::refused_error());
        }
        // A crawler whose manifest declares `*` reaches the host of the address it was given
        // and no other; one that named its domains keeps them. See `crawl_host`.
        let mut store =
            self.runtime
                .store_for(account, None, self.runtime.crawl_host(url).as_deref())?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let answer = instance
            .rdownloader_plugin_crawler()
            .call_crawl(&mut store, url)
            .await?;
        Ok(match answer {
            Ok(links) => Ok(links
                .into_iter()
                .take(MAX_CRAWLED_LINKS)
                .map(|link| CrawledLink {
                    url: link.url,
                    file_name: link.file_name,
                    size: link.size,
                    package_hint: link.package_hint,
                    mirror_hint: None,
                })
                .collect()),
            Err(failure) => Err(CrawlRefusal {
                not_mine: matches!(
                    failure.category,
                    crate::component::rdownloader::plugin::types::FailureKind::Unsupported
                ),
                code: failure.code,
                message: failure.message,
            }),
        })
    }
}
