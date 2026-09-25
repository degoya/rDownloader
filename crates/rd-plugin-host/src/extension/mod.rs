//! Host side of the extension plugin types (RD-090-12 … RD-090-17, RD-107-06, RD-110-33).
//!
//! Each type is one world in the same WIT package, so the generated types are shared and the
//! difference between them is only which interfaces the world imports and exports. One file
//! per type holds the wrapper the adapter crate calls; what they all rest on — compiling a
//! package, linking exactly the granted interfaces and nothing else, and building a store
//! that reaches only the declared domains — lives here, once.

pub mod bindings;
pub mod source;

mod auth;
mod crawler;
mod enricher;
mod intake;
mod notifier;
mod oauth;
mod postprocess;
mod remote_job;
mod storage;
mod stream_transform;

use std::sync::Arc;

use anyhow::Result;
use rd_core::AccountId;
use rd_plugin_api::{ClientIdentity, ResolverHost};
use wasmtime::{
    Store,
    component::{HasSelf, InstancePre, Linker},
};

pub use auth::{AuthProgress, AuthProvider};
pub use crawler::{CrawlRefusal, CrawledLink, FolderCrawler, MAX_CRAWLED_LINKS};
pub use enricher::MetadataEnricher;
pub use intake::{IntakeParser, IntakeProposal};
pub use notifier::{Delivery, NotifierPlugin};
pub use oauth::{AuthorizationRequest, DeviceAuthorization, OAuthProvider, TokenOutcome};
pub use postprocess::{PostprocessPlugin, StepOutcome};
pub use remote_job::{
    CacheAnswer, CacheKind, CacheQuery, CacheState, MAX_CACHE_CONTAINER_BYTES, MAX_CACHE_QUERIES,
    MAX_JOB_ARTIFACTS, MAX_JOB_ENTRIES, RemoteJobArtifact, RemoteJobEntry, RemoteJobHandle,
    RemoteJobPlugin, RemoteJobProgress, RemoteJobRefusal, RemoteJobSource, RemoteJobWork,
};
pub use source::SourceState;
pub use storage::{StoragePlugin, Upload, UploadOutcome};
pub use stream_transform::{
    MAX_TRANSFORM_BOUNDARIES, StreamTransformInfo, StreamTransformProvider,
    StreamTransformProviders, TransformedDownload,
};

use crate::{PluginManifest, PluginType, SandboxEngine, runtime::PluginStoreState};

/// Compiles a package of an extension type and links the world its manifest names.
///
/// Used by the conformance kit, which needs to know that the component actually satisfies
/// the world it claims — a package that declares `plugin_type = "notifier"` and exports an
/// intake interface would otherwise only fail the first time someone sends a notification.
pub fn instantiate(manifest: &PluginManifest, component_bytes: &[u8]) -> Result<()> {
    let sandbox = SandboxEngine::new(manifest.limits)?;
    let component = sandbox.compile_component(component_bytes, manifest)?;
    let linker = linker_for(&sandbox, manifest)?;
    let pre = linker.instantiate_pre(&component)?;
    // Building the typed pre-instance is what proves the export side: a component that does
    // not offer the world's interface fails here rather than at first use.
    typed_pre(&manifest.plugin_type, pre)
}

/// Checks the export side for one type, discarding the typed pre-instance.
fn typed_pre(plugin_type: &PluginType, pre: InstancePre<PluginStoreState>) -> Result<()> {
    match *plugin_type {
        PluginType::Intake => bindings::intake::IntakePluginPre::new(pre).map(|_| ()),
        PluginType::Auth => bindings::auth::AuthPluginPre::new(pre).map(|_| ()),
        PluginType::OAuth => bindings::oauth::OauthPluginPre::new(pre).map(|_| ()),
        PluginType::Crawler => bindings::crawler::CrawlerPluginPre::new(pre).map(|_| ()),
        PluginType::Enricher => bindings::enricher::EnricherPluginPre::new(pre).map(|_| ()),
        PluginType::Notifier => bindings::notifier::NotifierPluginPre::new(pre).map(|_| ()),
        PluginType::Postprocess => {
            bindings::postprocess::PostprocessPluginPre::new(pre).map(|_| ())
        }
        PluginType::Storage => bindings::storage::StoragePluginPre::new(pre).map(|_| ()),
        PluginType::RemoteJob => bindings::remote_job::RemoteJobPluginPre::new(pre).map(|_| ()),
        PluginType::StreamTransform => {
            bindings::stream_transform::StreamTransformPluginPre::new(pre).map(|_| ())
        }
        ref other => anyhow::bail!("{} is not an extension type", other.as_str()),
    }?;
    Ok(())
}

/// Builds a linker with exactly the interfaces a manifest grants.
///
/// Shared by every extension wrapper so the rule that decides what a plugin can reach lives
/// in one place: six copies of this would be six chances for one of them to link an
/// interface the manifest never asked for.
fn linker_for(
    sandbox: &SandboxEngine,
    manifest: &PluginManifest,
) -> Result<Linker<PluginStoreState>> {
    let mut linker: Linker<PluginStoreState> = Linker::new(sandbox.engine());
    crate::component::rdownloader::plugin::host::add_to_linker::<_, HasSelf<_>>(
        &mut linker,
        |state| state,
    )?;
    if manifest.capabilities.net_http.is_some() {
        crate::component::rdownloader::plugin::http::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )?;
    }
    if manifest.capabilities.net_stream.is_some() {
        crate::transfer::rdownloader::plugin::net::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )?;
    }
    if matches!(
        manifest.plugin_type,
        PluginType::Postprocess | PluginType::Storage
    ) {
        bindings::postprocess::rdownloader::plugin::source::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )?;
    }
    // Two grants no extension type needed before the crawler: a folder behind a sign-in is
    // reached with the account's cookies, and a share behind a challenge with the captcha
    // broker. Linked from the manifest rather than from the type, exactly as the resolver
    // does it, so what a plugin can reach stays the list it declared.
    if manifest.capabilities.cookies {
        crate::component::rdownloader::plugin::cookies::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )?;
    }
    if manifest.capabilities.captcha {
        crate::component::rdownloader::plugin::captcha::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )?;
    }
    // Computing over a credential the guest never sees (RD-120-20). A declared grant like
    // any other, and hand-wired rather than generated because the charge it makes needs the
    // store and not the store's data.
    if manifest.capabilities.key_derivation {
        crate::keyderive::add_to_linker(&mut linker)?;
    }
    // Writing a token back is not a capability but a definition: it is what an authentication
    // plugin does, and no other type can even name the interface.
    if matches!(manifest.plugin_type, PluginType::Auth | PluginType::OAuth) {
        bindings::auth::rdownloader::plugin::credentials::add_to_linker::<_, HasSelf<_>>(
            &mut linker,
            |state| state,
        )?;
    }
    Ok(linker)
}

/// What every extension wrapper is made of.
///
/// The wrappers differ only in which typed pre-instance they hold and which calls they make;
/// compiling, confining and giving an invocation its store is identical for all six, and a
/// per-type copy of it would be a per-type chance to forget the domain list or the host.
pub(crate) struct ExtensionRuntime {
    manifest: PluginManifest,
    sandbox: SandboxEngine,
    /// The application's own capabilities, already narrowed to this plugin's manifest. `None`
    /// leaves the plugin without a way out: it computes, and nothing else.
    host: Option<Arc<dyn ResolverHost>>,
}

impl ExtensionRuntime {
    /// Compiles a verified package and links only what its manifest grants.
    pub(crate) fn build(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<(Self, InstancePre<PluginStoreState>)> {
        let sandbox = SandboxEngine::new(manifest.limits)?;
        let component = sandbox.compile_component(component_bytes, &manifest)?;
        let linker = linker_for(&sandbox, &manifest)?;
        let pre = linker.instantiate_pre(&component)?;
        // The caller hands over the application's own host; narrowing it to this manifest is
        // done here rather than there, so no adapter can forget to do it.
        let host = host.map(|host| crate::native::GrantedHost::confine(host, &manifest));
        Ok((
            Self {
                manifest,
                sandbox,
                host,
            },
            pre,
        ))
    }

    /// The manifest this plugin was built from.
    pub(crate) fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// A store for one invocation.
    ///
    /// `account` is the account this invocation runs for, if any. The host compares it with
    /// every credential request, so an invocation started for one account cannot reach
    /// another's secret even when the plugin asks for it by name.
    pub(crate) fn store(&self, account: Option<AccountId>) -> Result<Store<PluginStoreState>> {
        self.store_with_secret(account, None)
    }

    /// A store that may expand exactly one vault reference into `{{secret}}`.
    ///
    /// The types with no provider account behind them — a notification destination, a storage
    /// target — reach their credential this way: the host says which one, the plugin writes
    /// `{{secret}}` and never learns what it stands for or where it came from.
    pub(crate) fn store_with_secret(
        &self,
        account: Option<AccountId>,
        granted_secret: Option<String>,
    ) -> Result<Store<PluginStoreState>> {
        self.store_for(account, granted_secret, None)
    }

    /// A store whose reachable addresses are narrowed to one host.
    ///
    /// A storage destination declares `*` in its manifest, because where somebody put their
    /// server is not something a plugin author can know. The grant that actually applies is
    /// this one: the host of the destination this upload is for, and nothing else. The
    /// manifest's `*` therefore reads as "wherever you configure", not as "anywhere".
    pub(crate) fn store_for(
        &self,
        account: Option<AccountId>,
        granted_secret: Option<String>,
        only_host: Option<&str>,
    ) -> Result<Store<PluginStoreState>> {
        self.store_reaching(account, granted_secret, self.reachable(only_host))
    }

    /// A store that reaches exactly `domains`.
    ///
    /// For a caller that has already decided the reach from something only it can read — a
    /// notification destination's address (RD-130-15). Whatever it passes is the whole list:
    /// nothing from the manifest is added back.
    pub(crate) fn store_reaching(
        &self,
        account: Option<AccountId>,
        granted_secret: Option<String>,
        domains: Vec<String>,
    ) -> Result<Store<PluginStoreState>> {
        self.sandbox.create_extension_store(
            domains,
            self.host.clone(),
            identity(account),
            granted_secret,
            self.writes(),
        )
    }

    /// Whether this plugin's type may use the methods that write at the far end.
    fn writes(&self) -> bool {
        self.manifest.plugin_type == PluginType::Storage
    }

    /// The single host one crawl may reach, for a plugin whose manifest declares `*`.
    ///
    /// A self-hosted service has no domain that could stand in a manifest: a Nextcloud is
    /// wherever somebody put it, and an open directory index is any web server at all. The
    /// pattern is the storage destination's, one step further: the manifest's `*` reads as
    /// "wherever the address points", and the grant that actually applies is the host of the
    /// address this crawl was given. Because the same list is what the redirect check uses,
    /// a redirect off that host is refused exactly as it was before.
    ///
    /// `None` for a manifest that named its domains — `premiumize-crawler` crawls
    /// `premiumize.me` and fetches `www.premiumize.me`, so narrowing to the crawled host
    /// would take away a domain it declared and was granted.
    pub(crate) fn crawl_host(&self, url: &str) -> Option<String> {
        crawl_host(self.manifest.capabilities.domains(), url)
    }

    /// The domains one invocation may reach: the manifest's list, or the single host an
    /// invocation was pointed at, whichever is narrower.
    fn reachable(&self, only_host: Option<&str>) -> Vec<String> {
        match only_host {
            Some(host) => vec![host.to_ascii_lowercase()],
            None => self.manifest.capabilities.domains().to_vec(),
        }
    }

    /// A store for one invocation that reads the files of a package.
    pub(crate) fn source_store(
        &self,
        account: Option<AccountId>,
        granted_secret: Option<String>,
        only_host: Option<&str>,
        source: SourceState,
    ) -> Result<Store<PluginStoreState>> {
        self.sandbox.create_source_store(
            self.reachable(only_host),
            self.host.clone(),
            identity(account),
            granted_secret,
            self.writes(),
            source,
        )
    }
}

fn identity(account_id: Option<AccountId>) -> ClientIdentity {
    ClientIdentity {
        account_id,
        proxy_profile_id: None,
        tls_revision: 0,
    }
}

/// The host a `*` crawl narrows to: the host of the crawled address, or nothing when the
/// manifest named its domains instead.
///
/// Free-standing so the decision can be read and tested without a compiled component.
pub(crate) fn crawl_host(domains: &[String], url: &str) -> Option<String> {
    if !domains.iter().any(|domain| domain == "*") {
        return None;
    }
    let url = url::Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    url.host_str().map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::crawl_host;

    fn domains(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// RD-107-05, host gap 2: a crawler that declared `*` reaches the address it was given,
    /// and that host alone — the wildcard is "wherever the address points", not "anywhere".
    #[test]
    fn a_wildcard_crawl_narrows_to_the_host_it_was_given() {
        let wildcard = domains(&["*"]);
        assert_eq!(
            crawl_host(&wildcard, "https://cloud.example.org/s/abc123").as_deref(),
            Some("cloud.example.org")
        );
        // The host is compared lowercase everywhere else, so it is lowered here too.
        assert_eq!(
            crawl_host(&wildcard, "https://Cloud.Example.ORG/pub/").as_deref(),
            Some("cloud.example.org")
        );
        // A port does not widen the grant to a second host.
        assert_eq!(
            crawl_host(&wildcard, "https://cloud.example.org:8443/pub/").as_deref(),
            Some("cloud.example.org")
        );
        // Nothing that is not a web address narrows to anything.
        assert_eq!(crawl_host(&wildcard, "ftp://files.example.org/pub/"), None);
        assert_eq!(crawl_host(&wildcard, "not a url"), None);
    }

    /// A crawler that named its domains keeps them. `premiumize-crawler` is asked about
    /// `premiumize.me` and fetches `www.premiumize.me`; narrowing would take away a domain
    /// its manifest declared and the installer granted.
    #[test]
    fn a_named_domain_list_is_not_narrowed_to_the_crawled_host() {
        let named = domains(&["www.premiumize.me", "premiumize.me"]);
        assert_eq!(crawl_host(&named, "https://premiumize.me/folder/abc"), None);
        // Even a list that merely contains a sub-domain wildcard is a named list.
        assert_eq!(
            crawl_host(&domains(&["*.example.org"]), "https://a.example.org/x/"),
            None
        );
    }
}
