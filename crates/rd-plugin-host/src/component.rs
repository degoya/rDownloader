//! Wasmtime Component Model adapter for the public resolver WIT world.

use std::sync::Arc;

use async_trait::async_trait;
use rand::Rng;
use rd_core::{AccountId, ByteCount, Failure, FailureKind, LinkStatus, PluginLinkCheck};
use rd_plugin_api::{
    AccountStatus, CheckRequest, ClientIdentity, ResolvedDownload, Resolver, ResolverHost,
    ResolverMetadata,
};
use url::Url;
use wasmtime::component::{HasSelf, Linker};

use crate::{PluginManifest, SandboxEngine, domain_allowed, runtime::PluginStoreState};

wasmtime::component::bindgen!({
    path: "../rd-plugin-api/wit",
    world: "resolver-plugin",
    imports: { default: async },
    exports: { default: async },
});

use rdownloader::plugin::{
    captcha as wit_captcha, cookies as wit_cookies, host as wit_host, http as wit_http,
    types as wit_types,
};

mod convert;
mod host_impls;
#[cfg(test)]
mod redaction_tests;

use convert::permanent;
pub(crate) use convert::{
    component_failure, from_wit_download, from_wit_failure, to_wit_failure, to_wit_identity,
};

/// Smallest reservation worth making for a captcha; below this no service answers and
/// nobody types in time, so the plugin is told its budget is gone instead.
const MIN_CAPTCHA_ALLOWANCE: std::time::Duration = std::time::Duration::from_secs(30);
/// Longest accepted captcha site key; real ones are far shorter.
const MAX_SITE_KEY_BYTES: usize = 256;
/// Longest accepted captcha image, well above any real one.
const MAX_CAPTCHA_IMAGE_BYTES: usize = 512 * 1024;
/// Most random bytes one `host.random-bytes` call may ask for.
///
/// A PKCE verifier needs 32 and the largest thing a plugin plausibly seeds is a key; a request
/// beyond this is a mistake or an attempt to make the host allocate, and either is answered
/// with nothing rather than served.
const MAX_RANDOM_BYTES: u32 = 1024;

/// A compiled, version-specific Component implementing the native resolver abstraction.
pub struct ComponentResolver {
    metadata: ResolverMetadata,
    host_domains: Vec<String>,
    download_domains: Vec<String>,
    sandbox: SandboxEngine,
    pre: ResolverPluginPre<PluginStoreState>,
    host: Arc<dyn ResolverHost>,
    log: Arc<crate::ExecutionLog>,
}

impl ComponentResolver {
    /// Compiles a verified package and pins its manifest version for this resolver instance.
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Arc<dyn ResolverHost>,
    ) -> anyhow::Result<Self> {
        let sandbox = SandboxEngine::new(manifest.limits)?;
        let component = sandbox.compile_component(component_bytes, &manifest)?;
        let mut linker = Linker::new(sandbox.engine());
        // One grant, one interface. Only what the manifest asks for is linked, so a
        // component that imports an interface it was not granted fails to instantiate
        // instead of being turned away later, at a call the user is already waiting on.
        rdownloader::plugin::host::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        if manifest.capabilities.net_http.is_some() {
            rdownloader::plugin::http::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        }
        if manifest.capabilities.cookies {
            rdownloader::plugin::cookies::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| {
                state
            })?;
        }
        if manifest.capabilities.captcha {
            rdownloader::plugin::captcha::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| {
                state
            })?;
        }
        let pre = ResolverPluginPre::new(linker.instantiate_pre(&component)?)?;
        let host_domains = manifest.domains().to_vec();
        let provider_slug = manifest.message_slug().to_owned();
        let match_domains = if manifest.match_domains.is_empty() {
            host_domains.clone()
        } else {
            manifest.match_domains
        };
        let download_domains = if manifest.download_domains.is_empty() {
            host_domains.clone()
        } else {
            manifest.download_domains
        };
        let metadata = ResolverMetadata {
            plugin_id: manifest.id,
            name: manifest.name,
            version: manifest.version,
            provider_slug,
            domains: match_domains,
            max_concurrent_downloads: manifest.max_concurrent_downloads,
            requires_account: manifest.requires_account,
        };
        Ok(Self {
            metadata,
            host_domains,
            download_domains,
            sandbox,
            pre,
            host,
            log: crate::ExecutionLog::disabled(),
        })
    }

    /// Records this resolver's invocations in the plugin execution history.
    #[must_use]
    pub fn with_execution_log(mut self, log: Arc<crate::ExecutionLog>) -> Self {
        self.log = log;
        self
    }

    /// Asks the guest whether it claims `url`.
    ///
    /// The host's own `matches` only consults the manifest; this is the guest's answer, and
    /// the two disagreeing is a plugin that link intake routes work to which it then refuses.
    pub async fn guest_claims(&self, url: &str) -> Result<bool, Failure> {
        if crate::foreign_address::carries_marker(url) {
            return Ok(false);
        }
        let identity = ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        };
        let mut store = self.store(identity)?;
        let bindings = self.bindings(&mut store).await?;
        bindings
            .rdownloader_plugin_resolver()
            .call_match_url(&mut store, url)
            .await
            .map_err(component_failure)
    }

    /// Times one call and files what became of it.
    async fn recorded<T, F>(&self, operation: &'static str, call: F) -> Result<T, Failure>
    where
        F: std::future::Future<Output = Result<T, Failure>>,
    {
        let invocation = self.log.begin(
            &self.metadata.plugin_id.to_string(),
            &self.metadata.name,
            &self.metadata.version,
            "resolver",
            operation,
        );
        let result = call.await;
        self.log.finish(invocation, &result);
        result
    }

    async fn bindings(
        &self,
        store: &mut wasmtime::Store<PluginStoreState>,
    ) -> Result<ResolverPlugin, Failure> {
        self.pre
            .instantiate_async(store)
            .await
            .map_err(component_failure)
    }

    fn store(
        &self,
        identity: ClientIdentity,
    ) -> Result<wasmtime::Store<PluginStoreState>, Failure> {
        self.sandbox
            .create_invocation_store(self.host_domains.clone(), Arc::clone(&self.host), identity)
            .map_err(|error| {
                Failure::coded(
                    FailureKind::Permanent,
                    "plugin.execution_failed",
                    format!("Plugin execution failed: {error:#}"),
                )
            })
    }
}

#[async_trait]
impl Resolver for ComponentResolver {
    fn metadata(&self) -> &ResolverMetadata {
        &self.metadata
    }

    fn matches(&self, url: &Url) -> bool {
        domain_allowed(url, &self.metadata.domains)
    }

    async fn check_account(&self, account_id: AccountId) -> Result<AccountStatus, Failure> {
        self.recorded("check_account", self.check_account_inner(account_id))
            .await
    }

    async fn resolve(
        &self,
        request: rd_plugin_api::ResolveRequest,
    ) -> Result<ResolvedDownload, Failure> {
        self.recorded("resolve", self.resolve_inner(request)).await
    }

    async fn hosters(&self, account_id: AccountId) -> Result<Vec<String>, Failure> {
        self.recorded("hosters", self.hosters_inner(account_id))
            .await
    }

    async fn check(&self, request: CheckRequest) -> Result<Vec<PluginLinkCheck>, Failure> {
        self.recorded("check", self.check_inner(request)).await
    }
}

/// The guest-facing bodies. Split out so every one of them is timed and recorded by the
/// trait method above rather than by remembering to do it in each.
impl ComponentResolver {
    async fn check_account_inner(&self, account_id: AccountId) -> Result<AccountStatus, Failure> {
        let identity = ClientIdentity {
            account_id: Some(account_id),
            proxy_profile_id: None,
            tls_revision: 0,
        };
        let mut store = self.store(identity)?;
        let bindings = self.bindings(&mut store).await?;
        let result = bindings
            .rdownloader_plugin_resolver()
            .call_check_account(&mut store, &account_id.to_string())
            .await
            .map_err(component_failure)?
            .map_err(from_wit_failure)?;
        Ok(AccountStatus {
            valid: result.valid,
            premium: result.premium,
            label: crate::account_label::from_wit_label(result.label)?,
            traffic_left: result
                .traffic_left
                .map(ByteCount::new)
                .transpose()
                .map_err(permanent)?,
        })
    }

    async fn resolve_inner(
        &self,
        request: rd_plugin_api::ResolveRequest,
    ) -> Result<ResolvedDownload, Failure> {
        if !self.matches(&request.url) {
            return Err(Failure::coded(
                FailureKind::Unsupported,
                "plugin.url_outside_domains",
                "URL is outside the plugin domains",
            ));
        }
        if crate::foreign_address::carries_marker(request.url.as_str()) {
            return Err(crate::foreign_address::refused());
        }
        let expected_identity = request.client.clone();
        let input_url = request.url.to_string();
        let input = wit_types::ResolveRequest {
            url: input_url.clone(),
            client: to_wit_identity(&request.client),
        };
        let mut store = self.store(request.client)?;
        let bindings = self.bindings(&mut store).await?;
        let guest = bindings.rdownloader_plugin_resolver();
        if !guest
            .call_match_url(&mut store, &input_url)
            .await
            .map_err(component_failure)?
        {
            return Err(Failure::coded(
                FailureKind::Unsupported,
                "plugin.url_rejected",
                "Plugin rejected the input URL",
            ));
        }
        let result = guest
            .call_resolve(&mut store, &input)
            .await
            .map_err(component_failure)?
            .map_err(from_wit_failure)?;
        from_wit_download(result, expected_identity, &self.download_domains)
    }

    async fn hosters_inner(&self, account_id: AccountId) -> Result<Vec<String>, Failure> {
        let identity = ClientIdentity {
            account_id: Some(account_id),
            proxy_profile_id: None,
            tls_revision: 0,
        };
        let mut store = self.store(identity)?;
        let bindings = self.bindings(&mut store).await?;
        let hosters = bindings
            .rdownloader_plugin_resolver()
            .call_hosters(&mut store, &account_id.to_string())
            .await
            .map_err(component_failure)?
            .map_err(from_wit_failure)?;
        Ok(hosters
            .into_iter()
            .filter(|host| host.len() <= 253 && !host.is_empty())
            .take(10_000)
            .collect())
    }

    async fn check_inner(&self, request: CheckRequest) -> Result<Vec<PluginLinkCheck>, Failure> {
        let (urls, mut unknown) = crate::foreign_address::checkable(request.urls);
        if urls.is_empty() {
            return Ok(unknown);
        }
        let requested: Vec<String> = urls.iter().map(ToString::to_string).collect();
        let input = wit_types::CheckRequest {
            urls: requested.clone(),
            client: to_wit_identity(&request.client),
        };
        let mut store = self.store(request.client)?;
        let bindings = self.bindings(&mut store).await?;
        let results = bindings
            .rdownloader_plugin_resolver()
            .call_check(&mut store, &input)
            .await
            .map_err(component_failure)?
            .map_err(from_wit_failure)?;
        let mut checked = results
            .into_iter()
            .map(|result| {
                if !requested.contains(&result.url) {
                    return Err(Failure::coded(
                        FailureKind::Permanent,
                        "plugin.unexpected_url",
                        "Plugin reported a URL that was not requested",
                    ));
                }
                Ok(PluginLinkCheck {
                    url: Url::parse(&result.url).map_err(permanent)?,
                    status: match result.status {
                        wit_types::LinkStatus::Online => LinkStatus::Online,
                        wit_types::LinkStatus::Offline => LinkStatus::Offline,
                        wit_types::LinkStatus::Unknown => LinkStatus::Unknown,
                        wit_types::LinkStatus::Cached => LinkStatus::Cached,
                    },
                    file_name: result.file_name,
                    size: result
                        .size
                        .map(ByteCount::new)
                        .transpose()
                        .map_err(permanent)?,
                })
            })
            .collect::<Result<Vec<_>, Failure>>()?;
        checked.append(&mut unknown);
        Ok(checked)
    }
}

/// The host's answer to `host.random-bytes`: `count` bytes from the operating system's
/// generator, or nothing at all.
///
/// Refusing an oversized request with an empty list rather than a truncated one matters: a
/// guest that is handed fewer bytes than it asked for must notice, and silently shortening the
/// answer is exactly how a 32-byte verifier turns into an 8-byte one nobody spots.
fn random_bytes(count: u32) -> Vec<u8> {
    if count == 0 || count > MAX_RANDOM_BYTES {
        return Vec::new();
    }
    let mut bytes = vec![0u8; count as usize];
    // `rand::rng()` is the OS-seeded, periodically reseeded CSPRNG the rest of the application
    // draws its keys and nonces from; nothing here is derived from a clock or an identifier.
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

#[cfg(test)]
#[path = "component_tests.rs"]
mod tests;
