//! Streams the host has to transform before they are files (RD-110-33, ADR 0011).
//!
//! The twelfth world, and the only one whose answer the host *computes with* rather than
//! merely records. A provider that encrypts on the client keeps the key out of its own reach:
//! it rides in a link fragment, and what a plain `GET` returns is ciphertext. The resolver
//! world cannot carry that -- `resolved-download` is an address and headers -- and a header
//! would be logged and replayed on every chunk request.
//!
//! So this wrapper takes in an address, a declarative description of the transform, and the
//! key. What it hands on is the description and [`rd_core::TransformKey`], whose `Debug`
//! prints a placeholder and which has no `Serialize` at all: from here the bytes go to the
//! vault and the description keeps only the reference. The description itself is validated
//! before it leaves this module, so a primitive this build does not implement is a refusal
//! with a stable code rather than a download that computes something wrong.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use rd_core::{ContentTransform, Failure, FailureKind, TransformKey, redact_text};
use rd_plugin_api::{ClientIdentity, ResolveRequest, ResolvedDownload, ResolverHost};

use super::{ExtensionRuntime, bindings::stream_transform};
use crate::{
    PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry, runtime::PluginStoreState,
};

/// The most chunk boundaries a plugin's answer may carry before it is refused outright.
///
/// The list arrives from a plugin, so it is bounded rather than trusted; `rd-core` holds the
/// same limit and refuses again when the description is validated.
pub const MAX_TRANSFORM_BOUNDARIES: usize = rd_core::MAX_BOUNDARIES;

/// A compiled stream-transform plugin, pinned to one installed manifest version.
pub struct StreamTransformProvider {
    runtime: ExtensionRuntime,
    pre: stream_transform::StreamTransformPluginPre<PluginStoreState>,
}

/// One address, how its bytes become a file, and the key that does it.
///
/// The key is separate from the description on purpose: the description is written down, the
/// key is put in the vault and replaced by a reference.
#[derive(Debug)]
pub struct TransformedDownload {
    pub download: ResolvedDownload,
    pub transform: ContentTransform,
    pub key: TransformKey,
}

impl StreamTransformProvider {
    /// Compiles a verified package and links only what its manifest grants.
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: stream_transform::StreamTransformPluginPre::new(pre)?,
        })
    }

    /// The manifest this plugin was built from.
    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// Whether the plugin claims this address. Reaches nothing; the guest answers from the
    /// address alone, so a link belonging to somebody else is never fetched on its behalf.
    pub async fn claims(&self, url: &str) -> Result<bool> {
        if crate::foreign_address::carries_marker(url) {
            return Ok(false);
        }
        let mut store = self.runtime.store(None)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        Ok(instance
            .rdownloader_plugin_stream_transform()
            .call_claims_url(&mut store, url)
            .await?)
    }

    /// Asks for the address and the transform description behind it.
    ///
    /// Everything a plugin says about the transform is checked here, before any byte is
    /// fetched: an unknown primitive, a nonce of the wrong length, a boundary list that does
    /// not ascend. A refusal carries the stable code the interface translates.
    pub async fn resolve(
        &self,
        request: &ResolveRequest,
    ) -> Result<Result<TransformedDownload, Failure>> {
        if crate::foreign_address::carries_marker(request.url.as_str()) {
            return Ok(Err(crate::foreign_address::refused()));
        }
        let mut store = self.runtime.store(request.client.account_id)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let wit_request = stream_transform::rdownloader::plugin::types::ResolveRequest {
            url: request.url.to_string(),
            client: crate::component::to_wit_identity(&request.client),
        };
        let answer = instance
            .rdownloader_plugin_stream_transform()
            .call_resolve(&mut store, &wit_request)
            .await?;
        let answer = match answer {
            Ok(answer) => answer,
            Err(failure) => {
                return Ok(Err(crate::component::from_wit_failure(failure)));
            }
        };
        Ok(self.accept(answer, request.client.clone()))
    }

    /// Turns the guest's answer into the host's types, refusing everything it cannot compute.
    fn accept(
        &self,
        answer: stream_transform::exports::rdownloader::plugin::stream_transform::TransformedDownload,
        client: ClientIdentity,
    ) -> Result<TransformedDownload, Failure> {
        let domains = self.runtime.manifest().capabilities.domains().to_vec();
        let download = crate::component::from_wit_download(answer.download, client, &domains)?;
        let boundaries = answer
            .transform
            .integrity
            .as_ref()
            .map_or(0, |integrity| integrity.boundaries.len());
        if boundaries > MAX_TRANSFORM_BOUNDARIES {
            return Err(Failure::coded(
                rd_core::FailureKind::Permanent,
                rd_core::CODE_PARAMETERS_INVALID,
                format!("a chunk boundary list of {boundaries} is past what the host accepts"),
            ));
        }
        let cipher = answer.transform.cipher;
        let key = TransformKey::new(cipher.key);
        let transform = ContentTransform {
            cipher: rd_core::CipherSpec {
                algorithm: cipher.algorithm,
                // Filled in by the caller once the bytes are in the vault; a description
                // never carries the key itself.
                key_reference: None,
                nonce: cipher.nonce,
                first_block: cipher.first_block,
            },
            integrity: answer
                .transform
                .integrity
                .map(|integrity| rd_core::IntegritySpec {
                    algorithm: integrity.algorithm,
                    boundaries: integrity.boundaries,
                    iv: integrity.iv,
                    expected: integrity.expected,
                }),
        };
        transform.validate()?;
        Ok(TransformedDownload {
            download,
            transform,
            key,
        })
    }
}

/// The installed stream-transform plugins, newest version of each.
pub struct StreamTransformProviders {
    plugins: Vec<Loaded>,
}

struct Loaded {
    manifest: PluginManifest,
    plugin: StreamTransformProvider,
}

/// What one installed plugin looks like to whoever is listing them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamTransformInfo {
    pub plugin_id: String,
    pub name: String,
    pub version: String,
}

impl StreamTransformProviders {
    /// Loads every installed stream-transform plugin, skipping any that fails to build.
    pub async fn load(
        installer: &PluginInstaller,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        Ok(Self::from_registry(
            &PluginTypeRegistry::load(installer).await?,
            host,
        ))
    }

    /// The same, from a registry the adapters share.
    #[must_use]
    pub fn from_registry(
        registry: &PluginTypeRegistry,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Self {
        let loaded = registry.instantiate(&PluginType::StreamTransform, |package| {
            StreamTransformProvider::new(package.manifest.clone(), &package.component, host.clone())
                .map(|plugin| Loaded {
                    manifest: package.manifest.clone(),
                    plugin,
                })
        });
        // The registry yields the newest version of each plugin first, so the first entry
        // for an id wins and an older version left on disk is ignored.
        let mut seen = HashMap::new();
        let mut plugins = Vec::new();
        for entry in loaded {
            if seen.insert(entry.manifest.id.to_string(), ()).is_none() {
                plugins.push(entry);
            }
        }
        Self { plugins }
    }

    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Every installed plugin, sorted by name so the list does not reshuffle itself.
    #[must_use]
    pub fn list(&self) -> Vec<StreamTransformInfo> {
        let mut entries: Vec<StreamTransformInfo> = self
            .plugins
            .iter()
            .map(|entry| StreamTransformInfo {
                plugin_id: entry.manifest.id.to_string(),
                name: entry.manifest.name.clone(),
                version: entry.manifest.version.clone(),
            })
            .collect();
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        entries
    }

    /// Whether any installed plugin claims this address.
    pub async fn claims(&self, url: &str) -> bool {
        for entry in &self.plugins {
            if entry.plugin.claims(url).await.unwrap_or(false) {
                return true;
            }
        }
        false
    }

    /// Asks the first plugin that claims the address what lies behind it.
    ///
    /// `Ok(None)` means no installed plugin claims it, which is the ordinary case for every
    /// address in the world; the caller then treats the link as a plain download.
    pub async fn resolve(
        &self,
        request: &ResolveRequest,
    ) -> Option<Result<TransformedDownload, Failure>> {
        let url = request.url.to_string();
        let mut refusal = None;
        for entry in &self.plugins {
            if !entry.plugin.claims(&url).await.unwrap_or(false) {
                continue;
            }
            match entry.plugin.resolve(request).await {
                Ok(Ok(answer)) => return Some(Ok(answer)),
                Ok(Err(failure)) => {
                    // A plugin that claimed the address and then found it was not its own
                    // must not end the link; the next claimer gets its turn.
                    if matches!(failure.category, FailureKind::Unsupported) {
                        continue;
                    }
                    return Some(Err(failure));
                }
                Err(error) => {
                    tracing::warn!(
                        plugin = %entry.manifest.id,
                        "stream-transform plugin failed: {}",
                        redact_text(&format!("{error:#}"))
                    );
                    refusal = Some(Failure::coded(
                        FailureKind::Transient {
                            retry_after_seconds: None,
                        },
                        "plugin.execution_failed",
                        "The plugin did not finish",
                    ));
                }
            }
        }
        refusal.map(Err)
    }
}
