use std::sync::Arc;

use rd_core::{AccountId, Failure, FailureKind, LinkCheckResult, ProxyProfileId, ResolverPin};
use rd_http::{ClientPool, SharedNetworkDefaults};
use rd_plugin_api::{
    AccountStatus, CheckRequest, ClientIdentity, ResolveRequest, ResolvedDownload, Resolver,
    ResolverHost,
};
use url::Url;

#[cfg(test)]
mod bundled_headers_tests;
mod expand;
pub(crate) mod granted;
mod host;
mod references;
mod signin;
mod transfer_auth;

pub use expand::{
    CLIENT_ID_MARKER, client_not_configured, provider_cookie_scope, provider_download_bearer,
    provider_token_beside_the_flow,
};
pub(crate) use granted::GrantedHost;
use host::NativeHost;
pub use transfer_auth::{provider_download_authorization, provider_download_carries_credential};

/// Native resolver chain backed by the same constrained host capabilities as Components.
#[derive(Clone)]
pub struct ResolverService {
    database: rd_db::Database,
    resolvers: Arc<Vec<Arc<dyn Resolver>>>,
    host: Arc<dyn ResolverHost>,
}

impl ResolverService {
    #[must_use]
    pub fn new(
        database: rd_db::Database,
        clients: ClientPool,
        secrets: rd_secrets::SecretStore,
        network_defaults: SharedNetworkDefaults,
        captcha: Option<Arc<dyn rd_plugin_api::CaptchaSolver>>,
    ) -> Self {
        let host: Arc<dyn ResolverHost> = Arc::new(NativeHost::new(
            database.clone(),
            clients,
            secrets,
            network_defaults,
            captcha,
        ));
        // Each built-in resolver sees the shared host only through its own manifest, so it
        // is confined exactly as the packaged component of the same plugin would be.
        let granted = |manifest: &str| GrantedHost::wrap(host.clone(), manifest);
        let resolvers: Vec<Arc<dyn Resolver>> = vec![
            Arc::new(rd_plugin_alldebrid::AllDebridResolver::new(granted(
                rd_plugin_alldebrid::MANIFEST,
            ))),
            Arc::new(rd_plugin_ddownload::DdownloadResolver::new(granted(
                rd_plugin_ddownload::MANIFEST,
            ))),
            Arc::new(rd_plugin_debridlink::DebridLinkResolver::new(granted(
                rd_plugin_debridlink::MANIFEST,
            ))),
            Arc::new(rd_plugin_filejoker::FilejokerResolver::new(granted(
                rd_plugin_filejoker::MANIFEST,
            ))),
            Arc::new(rd_plugin_hitfile::HitfileResolver::new(granted(
                rd_plugin_hitfile::MANIFEST,
            ))),
            Arc::new(rd_plugin_katfile::KatfileResolver::new(granted(
                rd_plugin_katfile::MANIFEST,
            ))),
            Arc::new(rd_plugin_keep2share::Keep2ShareResolver::new(granted(
                rd_plugin_keep2share::MANIFEST,
            ))),
            Arc::new(rd_plugin_krakenfiles::KrakenfilesResolver::new(granted(
                rd_plugin_krakenfiles::MANIFEST,
            ))),
            Arc::new(rd_plugin_linksnappy::LinkSnappyResolver::new(granted(
                rd_plugin_linksnappy::MANIFEST,
            ))),
            Arc::new(rd_plugin_mediafire::MediafireResolver::new(granted(
                rd_plugin_mediafire::MANIFEST,
            ))),
            Arc::new(rd_plugin_nitroflare::NitroflareResolver::new(granted(
                rd_plugin_nitroflare::MANIFEST,
            ))),
            Arc::new(rd_plugin_onefichier::OneFichierResolver::new(granted(
                rd_plugin_onefichier::MANIFEST,
            ))),
            Arc::new(rd_plugin_premiumize::PremiumizeResolver::new(granted(
                rd_plugin_premiumize::MANIFEST,
            ))),
            Arc::new(rd_plugin_rapidgator::RapidgatorResolver::new(granted(
                rd_plugin_rapidgator::MANIFEST,
            ))),
            Arc::new(rd_plugin_turbobit::TurbobitResolver::new(granted(
                rd_plugin_turbobit::MANIFEST,
            ))),
        ];
        Self {
            database,
            resolvers: Arc::new(resolvers),
            host,
        }
    }

    /// The application's own host capabilities, unnarrowed.
    ///
    /// Handed to the extension plugin types, which narrow it to their own manifest exactly as
    /// a resolver does. Nothing here is a grant by itself — every call through it is still
    /// checked against the manifest of whoever made it.
    #[must_use]
    pub fn host(&self) -> Arc<dyn ResolverHost> {
        Arc::clone(&self.host)
    }

    /// Re-verifies and loads installed Components, with each resolver pinning one exact version.
    ///
    /// Once the final resolver chain is known, jobs pinned to a version that is no longer in
    /// it are released. Anything else leaves those jobs failing with
    /// `plugin.pinned_version_missing` on every retry for the rest of their life.
    pub async fn load_installed_components(
        &mut self,
        installer: &crate::PluginInstaller,
    ) -> anyhow::Result<usize> {
        let registry = crate::PluginTypeRegistry::load(installer).await?;
        self.load_components_from_registry(&registry).await
    }

    /// The same, from a registry the adapters share.
    ///
    /// Building a registry re-verifies and compiles *every* installed package — an Ed25519
    /// check, a wasmparser validation, a `SandboxEngine` with its epoch-ticker thread and a
    /// compile each — so the one `load_installed_components` builds for itself is only worth it
    /// for a caller that loads nothing else. The resolvers were the last holder of such a
    /// private pass after the ten extension adapters moved onto the shared registry; a service
    /// that starts them all now pays for one.
    pub async fn load_components_from_registry(
        &mut self,
        registry: &crate::PluginTypeRegistry,
    ) -> anyhow::Result<usize> {
        let mut loaded = compatible_components(
            registry,
            Arc::clone(&self.host),
            crate::ExecutionLog::new(self.database.clone()),
        );
        let count = loaded.len();
        loaded.extend(self.resolvers.iter().cloned());
        self.resolvers = Arc::new(loaded);
        self.release_unsatisfiable_pins().await;
        Ok(count)
    }

    /// Frees jobs whose pinned resolver version this build cannot provide any more.
    async fn release_unsatisfiable_pins(&self) {
        let available = self
            .resolvers
            .iter()
            .map(|resolver| {
                let metadata = resolver.metadata();
                (metadata.plugin_id.to_string(), metadata.version.clone())
            })
            .collect();
        match self
            .database
            .clear_unsatisfiable_resolver_pins(available)
            .await
        {
            Ok(0) => {}
            Ok(freed) => tracing::info!(
                freed,
                "released downloads pinned to a resolver version this build no longer has"
            ),
            // Diagnostics for stale pins must not keep the service from starting; the jobs
            // simply stay pinned and report the missing version as they did before.
            Err(error) => {
                tracing::warn!(error = %error, "could not release stale resolver pins")
            }
        }
    }

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
        // Before any resolver, built in or installed, sees the link: a marker inside it would
        // be expanded with the account's credential (RD-120-66).
        if crate::foreign_address::carries_marker(url.as_str()) {
            return Err(crate::foreign_address::refused());
        }
        let Some(account_id) = account_id else {
            let resolver = self.resolvers.iter().find(|resolver| {
                let metadata = resolver.metadata();
                let pin_matches = pin.is_none_or(|pin| {
                    metadata.plugin_id == pin.plugin_id && metadata.version == pin.version
                });
                !metadata.requires_account && pin_matches && resolver.matches(&url)
            });
            let Some(resolver) = resolver else {
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
        self.resolvers
            .iter()
            .find(|resolver| !resolver.metadata().requires_account && resolver.matches(url))
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
        self.resolvers.iter().any(|resolver| resolver.matches(url))
    }

    /// Runs the provider resolver's redaction-safe account check.
    pub async fn check_account(&self, account_id: AccountId) -> Result<AccountStatus, Failure> {
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self
            .resolver_for_provider(&provider, None)?
            .ok_or_else(|| {
                Failure::coded(
                    FailureKind::Unsupported,
                    "plugin.resolver_missing",
                    "No resolver is installed for this provider",
                )
            })?;
        resolver.check_account(account_id).await
    }

    /// Hoster catalogue of the account's provider resolver.
    pub async fn hosters(&self, account_id: AccountId) -> Result<Vec<String>, Failure> {
        let provider = account_provider(&self.database, account_id).await?;
        let resolver = self
            .resolver_for_provider(&provider, None)?
            .ok_or_else(|| {
                Failure::coded(
                    FailureKind::Unsupported,
                    "plugin.resolver_missing",
                    "No resolver is installed for this provider",
                )
            })?;
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
        let resolver = self
            .resolver_for_provider(&provider, None)?
            .ok_or_else(|| {
                Failure::coded(
                    FailureKind::Unsupported,
                    "plugin.resolver_missing",
                    "No resolver is installed for this provider",
                )
            })?;
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

    fn resolver_for_provider(
        &self,
        provider: &str,
        pin: Option<&ResolverPin>,
    ) -> Result<Option<&Arc<dyn Resolver>>, Failure> {
        let resolver = self.resolvers.iter().find(|resolver| {
            let metadata = resolver.metadata();
            let provider_matches = metadata.provider_slug.eq_ignore_ascii_case(provider.trim());
            let pin_matches = pin.is_none_or(|pin| {
                metadata.plugin_id == pin.plugin_id && metadata.version == pin.version
            });
            provider_matches && pin_matches
        });
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

/// Classifies an unresolvable link: hoster links need a plugin to become transferable and
/// must fail loudly, while a plain HTTP URL is downloaded as it is.
///
/// The provider registry is the only authority consulted here. Asking the resolvers whether
/// one `matches()` the URL looks equivalent but is not: a multihoster accepts any host by
/// declaring a `*` domain, so every plain link would be reported as needing an account and
/// direct downloads would stop working entirely. `provider_for_url` claims a URL only for a
/// `Hoster`-kind provider that lists its host explicitly, which is exactly the question —
/// and installed plugins are covered too, since their manifests contribute registry rows.
fn account_required(url: &Url) -> Option<Failure> {
    let spec = rd_provider_registry::provider_for_url(url)?;
    let host = url.host_str().unwrap_or_default().to_owned();
    Some(
        Failure::coded(
            FailureKind::AuthRequired,
            "resolve.account_required",
            "This hoster needs an account or a free download plugin",
        )
        .with_param("host", host)
        .with_param("provider", spec.slug),
    )
}

/// Puts the bundled provider rows into the registry, once per test process.
///
/// Until RD-101-13 eleven of them were compiled into `rd-provider-registry`, so a unit test
/// asking whether `ddownload.com` may be requested simply got an answer. A provider now exists
/// only while its plugin is installed, so a test that needs one has to say so — which is also
/// the state these tests are meant to reproduce.
#[cfg(test)]
pub(crate) fn register_bundled_providers_for_tests() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let rows: Vec<_> = std::fs::read_dir(root)
            .expect("plugins directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("manifest.toml"))
            .filter(|path| path.is_file())
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .filter_map(|text| toml::from_str::<crate::PluginManifest>(&text).ok())
            .collect::<Vec<_>>();
        rd_provider_registry::replace_secret_fragment_hosts(
            rows.iter()
                .flat_map(|manifest| manifest.secret_fragment_domains.iter().cloned())
                .collect(),
        );
        let rows: Vec<_> = rows
            .iter()
            .filter_map(crate::provider_spec_from_manifest)
            .collect();
        assert!(!rows.is_empty(), "expected bundled provider rows");
        let rejected = rd_provider_registry::replace_dynamic(rows);
        assert!(rejected.is_empty(), "rejected rows: {rejected:?}");
    });
}

fn compatible_components(
    registry: &crate::PluginTypeRegistry,
    host: Arc<dyn ResolverHost>,
    log: Arc<crate::ExecutionLog>,
) -> Vec<Arc<dyn Resolver>> {
    registry.instantiate(&crate::PluginType::Resolver, |package| {
        let resolver = crate::ComponentResolver::new(
            package.manifest.clone(),
            &package.component,
            Arc::clone(&host),
        )?
        .with_execution_log(Arc::clone(&log));
        Ok(Arc::new(resolver) as Arc<dyn Resolver>)
    })
}

/// Enabled account's provider slug, shared by resolver dispatch and the host's secret gating.
pub(super) async fn account_provider(
    database: &rd_db::Database,
    id: AccountId,
) -> Result<String, Failure> {
    account_credentials(database, id)
        .await
        .map(|(provider, _)| provider)
}

/// Enabled account's provider slug together with the credential mode it stores.
///
/// The mode decides which of a two-mode provider's secret slots is live, so every gate that
/// asks "may this credential go here" needs it alongside the slug.
pub(super) async fn account_credentials(
    database: &rd_db::Database,
    id: AccountId,
) -> Result<(String, Option<rd_provider_registry::CredentialMode>), Failure> {
    database
        .list_accounts()
        .await
        .map_err(permanent)?
        .into_iter()
        .find(|account| account.id == id && account.enabled)
        .map(|account| (account.provider, account.credential_mode))
        .ok_or_else(|| {
            Failure::coded(
                FailureKind::AccountInvalid,
                "plugin.account_unavailable",
                "Account is not available",
            )
        })
}

/// Enabled account's stored username, for `{{username}}` template expansion.
pub(super) async fn account_username(
    database: &rd_db::Database,
    id: AccountId,
) -> Result<Option<String>, Failure> {
    database
        .list_accounts()
        .await
        .map_err(permanent)?
        .into_iter()
        .find(|account| account.id == id && account.enabled)
        .map(|account| account.username)
        .ok_or_else(|| {
            Failure::coded(
                FailureKind::AccountInvalid,
                "plugin.account_unavailable",
                "Account is not available",
            )
        })
}

pub(super) fn permanent(error: impl std::fmt::Display) -> Failure {
    Failure::new(FailureKind::Permanent, error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use rd_core::{AccountId, Failure};
    use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};

    use super::compatible_components;
    use crate::{PluginManifest, VerifiedPackage};

    struct UnusedHost;

    #[async_trait]
    impl ResolverHost for UnusedHost {
        async fn http_request(
            &self,
            _client: &ClientIdentity,
            _request: HostHttpRequest,
        ) -> Result<HostHttpResponse, Failure> {
            Err(Failure::new(
                rd_core::FailureKind::Permanent,
                "unexpected HTTP request",
            ))
        }

        async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
            false
        }
    }

    #[test]
    fn incompatible_installed_component_is_skipped() {
        let manifest: PluginManifest = toml::from_str(
            r#"manifest_version = 3
plugin_type = "resolver"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-0000000000ff"
name = "Outdated"
version = "0.1.0"
key_id = "fixture"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
max_concurrent_downloads = 1

[capabilities.net_http]
domains = ["example.test"]

[metadata]
description = "Outdated fixture"
author = "Fixture Author"

[provider]
slug = "outdated"
kind = "hoster"
credentials = "api_key"
"#,
        )
        .expect("manifest");
        let package = VerifiedPackage {
            manifest,
            manifest_bytes: Vec::new(),
            component: b"\0asm\x0d\0\x01\0".to_vec(),
            signature: None,
            locales: Vec::new(),
        };

        let loaded = compatible_components(
            &crate::PluginTypeRegistry::new(vec![package]),
            Arc::new(UnusedHost),
            crate::ExecutionLog::disabled(),
        );

        assert!(loaded.is_empty());
    }

    /// A hoster link that no resolver can serve must surface an error. Reporting "resolved
    /// to nothing" instead made the scheduler download the hoster's landing page and store
    /// it under the link's name as a finished download.
    #[test]
    fn an_unresolvable_hoster_link_is_reported_as_needing_an_account() {
        super::register_bundled_providers_for_tests();
        let url = "https://rapidgator.net/file/abc123/archive.rar"
            .parse()
            .expect("URL");

        let failure = super::account_required(&url).expect("hoster link must fail");

        assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
        assert_eq!(failure.code.as_deref(), Some("resolve.account_required"));
        assert_eq!(
            failure.params.get("host").map(String::as_str),
            Some("rapidgator.net")
        );
        assert_eq!(
            failure.params.get("provider").map(String::as_str),
            Some("rapidgator")
        );
    }

    /// Plain HTTP links are still downloaded exactly as they were added.
    ///
    /// A multihoster declares a `*` domain, so asking the resolvers whether one "matches"
    /// a URL answers yes for every link on the internet. Classifying on that basis made
    /// every direct download fail with "this hoster needs an account"; the registry is
    /// consulted instead, and it claims a URL only for a hoster that lists its host.
    #[test]
    fn a_direct_link_is_not_mistaken_for_a_hoster_link() {
        for direct in [
            "https://cdn.example.test/releases/tool.bin",
            "http://127.0.0.1:8080/payload.bin",
            "https://files.example.org/a/b/c.zip",
        ] {
            let url: url::Url = direct.parse().expect("URL");
            assert!(
                super::account_required(&url).is_none(),
                "{direct} must stay a direct download"
            );
        }
    }

    /// Resolver dispatch matches `metadata.provider_slug` against the account's provider
    /// slug, so a built-in resolver and its registry row must agree on that slug. A typo on
    /// either side would silently fall back to direct HTTP (`resolve()` returns `Ok(None)`)
    /// instead of erroring.
    ///
    /// Only one direction is asserted here: every registered resolver needs a registry row.
    /// The reverse used to hold as well, back when the rows were the eleven compiled into the
    /// binary and each had a native resolver beside it. Since RD-101-13 the rows come from
    /// plugin manifests, and a provider may legitimately be served by a component alone —
    /// `xfs_generic` is exactly that and has no native build. That every bundled manifest
    /// still contributes its row is asserted in `tests/bundled_providers.rs`.
    #[tokio::test]
    async fn every_registered_resolver_agrees_with_the_provider_registry() {
        super::register_bundled_providers_for_tests();
        let directory = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(directory.path().join("db.sqlite"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secret store");
        let service = super::ResolverService::new(
            database,
            rd_http::ClientPool::default(),
            secrets,
            Arc::new(tokio::sync::RwLock::new(rd_http::NetworkDefaults::default())),
            None,
        );

        for resolver in service.resolvers.iter() {
            let metadata = resolver.metadata();
            assert!(
                rd_provider_registry::by_slug(&metadata.provider_slug).is_some(),
                "resolver {:?} declares provider slug {:?}, which has no registry row",
                metadata.name,
                metadata.provider_slug
            );
        }
    }

    /// Every built-in resolver must report exactly what its own bundled manifest says.
    ///
    /// The two builds of a plugin used to describe themselves separately — a
    /// `ResolverMetadata` literal in `native.rs` and the `manifest.toml` the component ships
    /// — and they had already drifted: ddownload's native domain list was missing the CDN
    /// wildcard its manifest grants, and the native build reported the *workspace* version,
    /// so every core release invalidated each `ResolverPin` pointing at a native resolver.
    /// Both now read one file; this test is what keeps a stray `include_str!` or a renamed
    /// manifest field from quietly reintroducing the split.
    #[tokio::test]
    async fn every_built_in_resolver_reports_its_own_bundled_manifest() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(directory.path().join("db.sqlite"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secret store");
        let service = super::ResolverService::new(
            database,
            rd_http::ClientPool::default(),
            secrets,
            Arc::new(tokio::sync::RwLock::new(rd_http::NetworkDefaults::default())),
            None,
        );

        let plugins = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
        let mut checked = 0;
        for resolver in service.resolvers.iter() {
            let metadata = resolver.metadata();
            let manifest_path = std::fs::read_dir(&plugins)
                .expect("plugins directory")
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("manifest.toml"))
                .filter(|path| path.is_file())
                .find(|path| {
                    let text = std::fs::read_to_string(path).expect("manifest");
                    let manifest: crate::PluginManifest = toml::from_str(&text).expect("parse");
                    manifest.message_slug() == metadata.provider_slug
                })
                .unwrap_or_else(|| {
                    panic!(
                        "no bundled manifest for provider {}",
                        metadata.provider_slug
                    )
                });
            let text = std::fs::read_to_string(&manifest_path).expect("manifest");
            let expected = rd_plugin_api::metadata_from_manifest(&text);
            assert_eq!(
                metadata,
                &expected,
                "{} does not report its own manifest",
                manifest_path.display()
            );
            checked += 1;
        }
        assert_eq!(checked, service.resolvers.len());
    }

    /// The end of the wiring an account-less download depends on: a hoster whose resolver
    /// opted into free downloads must be found without an account, and a plain HTTP link
    /// must not be claimed by one. Without this, `resolve()` reports "account required" and
    /// the free flow is unreachable no matter how complete the plugin is.
    #[tokio::test]
    async fn a_free_capable_hoster_is_found_without_an_account() {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(directory.path().join("db.sqlite"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secret store");
        let service = super::ResolverService::new(
            database,
            rd_http::ClientPool::default(),
            secrets,
            Arc::new(tokio::sync::RwLock::new(rd_http::NetworkDefaults::default())),
            None,
        );

        let katfile: url::Url = "https://katfile.biz/abc123xyz/release.rar"
            .parse()
            .expect("URL");
        assert!(
            service.has_free_resolver(&katfile),
            "KatFile declares requires_account = false and must be reachable without an account"
        );

        let direct: url::Url = "https://cdn.example.test/tool.bin".parse().expect("URL");
        assert!(
            !service.has_free_resolver(&direct),
            "a plain HTTP link must stay a direct download"
        );
    }

    /// `resolve()` against the real resolver chain, which includes multihosters whose `*`
    /// domain matches every URL. A plain link must come back as "not resolved" so the
    /// scheduler downloads it directly; anything else breaks every direct download, which
    /// a unit test over an empty resolver list cannot show.
    #[tokio::test]
    async fn a_plain_link_resolves_to_nothing_even_though_multihosters_match_every_host() {
        super::register_bundled_providers_for_tests();
        let directory = tempfile::tempdir().expect("tempdir");
        let database = rd_db::Database::open(directory.path().join("db.sqlite"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secret store");
        let service = super::ResolverService::new(
            database,
            rd_http::ClientPool::default(),
            secrets,
            Arc::new(tokio::sync::RwLock::new(rd_http::NetworkDefaults::default())),
            None,
        );

        for direct in [
            "http://127.0.0.1:8792/payload.bin",
            "https://cdn.example.test/releases/tool.bin",
        ] {
            let url: url::Url = direct.parse().expect("URL");
            let resolved = service
                .resolve(url, None, None, None)
                .await
                .unwrap_or_else(|failure| panic!("{direct} must not fail: {failure}"));
            assert!(
                resolved.is_none(),
                "{direct} must fall through to a direct download"
            );
        }

        // A known hoster with no account still fails loudly rather than downloading its page.
        let hoster: url::Url = "https://rapidgator.net/file/abc/x.rar"
            .parse()
            .expect("URL");
        let failure = service
            .resolve(hoster, None, None, None)
            .await
            .expect_err("a hoster link needs a resolver");
        assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    }
}
