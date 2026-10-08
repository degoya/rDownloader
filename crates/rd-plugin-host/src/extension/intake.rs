//! Intake parsers (RD-090-12): text and URLs in, LinkGrabber candidates out.

use std::sync::Arc;

use anyhow::Result;
use rd_plugin_api::ResolverHost;
use wasmtime::component::InstancePre;

use super::{
    ExtensionRuntime,
    bindings::{intake, intake_mirrors},
};
use crate::{PluginManifest, runtime::PluginStoreState};

/// The `mirror-sets` interface's export name up to its version.
const MIRROR_SETS_EXPORT: &str = "rdownloader:plugin/mirror-sets@";

/// Whether the component exports `mirror-sets` at all, in any version.
fn exports_mirror_sets(pre: &InstancePre<PluginStoreState>) -> bool {
    pre.component()
        .component_type()
        .exports(pre.engine())
        .any(|(name, _)| name.starts_with(MIRROR_SETS_EXPORT))
}

/// A compiled intake parser, pinned to one installed manifest version.
pub struct IntakeParser {
    runtime: ExtensionRuntime,
    pre: intake::IntakePluginPre<PluginStoreState>,
    /// The same component seen through `intake-mirrors-plugin`, when it exports
    /// `mirror-sets` (RD-150-03). `None` for every parser that states no sources.
    mirrors: Option<intake_mirrors::IntakeMirrorsPluginPre<PluginStoreState>>,
}

impl IntakeParser {
    /// Compiles a verified package and links only what its manifest grants.
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        // Typing the pre-instance is what checks the exports, so a component without
        // `mirror-sets` fails here and is simply a parser without sources. One that does export
        // it, in a form this host cannot type, is a broken package and says so instead of
        // quietly losing its sources (PL-04).
        let mirrors = match intake_mirrors::IntakeMirrorsPluginPre::new(pre.clone()) {
            Ok(mirrors) => Some(mirrors),
            Err(error) => {
                if exports_mirror_sets(&pre) {
                    tracing::warn!(plugin = %runtime.manifest().id, %error, "an intake parser exports mirror-sets in a form this host does not accept; it is used without sources");
                }
                None
            }
        };
        Ok(Self {
            runtime,
            pre: intake::IntakePluginPre::new(pre)?,
            mirrors,
        })
    }

    /// The manifest this parser was built from.
    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// Asks the plugin for candidates, returning an empty list when it claims nothing.
    ///
    /// `claims` is asked first so a plugin that has nothing to do with the input never sees
    /// it: an intake parser for one site should not be handed every paste in full.
    pub async fn parse(&self, input: &str) -> Result<Vec<IntakeProposal>> {
        let mut store = self.runtime.store(None)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let guest = instance.rdownloader_plugin_intake();
        if !guest.call_claims(&mut store, input).await? {
            return Ok(Vec::new());
        }
        match guest.call_parse(&mut store, input).await? {
            Ok(candidates) => Ok(candidates
                .into_iter()
                .map(|candidate| IntakeProposal {
                    url: candidate.url,
                    file_name: candidate.file_name,
                    size: candidate.size,
                    package_hint: candidate.package_hint,
                })
                .collect()),
            Err(failure) => Err(refusal(failure)),
        }
    }

    /// Whether this parser states the sources of the files it proposes.
    #[must_use]
    pub fn states_sources(&self) -> bool {
        self.mirrors.is_some()
    }

    /// Asks the plugin for every source of every file in `input` (RD-150-03).
    ///
    /// Empty for a parser without `mirror-sets` and for input it does not claim. What comes
    /// back is unchecked; `rd_core::SourceSet::checked` is where it becomes something the
    /// queue may use.
    pub async fn source_sets(&self, input: &str) -> Result<Vec<SourceSetProposal>> {
        let Some(pre) = &self.mirrors else {
            return Ok(Vec::new());
        };
        let mut store = self.runtime.store(None)?;
        let instance = pre.instantiate_async(&mut store).await?;
        if !instance
            .rdownloader_plugin_intake()
            .call_claims(&mut store, input)
            .await?
        {
            return Ok(Vec::new());
        }
        match instance
            .rdownloader_plugin_mirror_sets()
            .call_sets(&mut store, input)
            .await?
        {
            Ok(sets) => Ok(sets
                .into_iter()
                .map(|set| SourceSetProposal {
                    primary_url: set.primary_url,
                    file_name: set.file_name,
                    size: set.size,
                    sources: set
                        .sources
                        .into_iter()
                        .map(|source| (source.url, source.priority, source.location))
                        .collect(),
                    hashes: set
                        .hashes
                        .into_iter()
                        .map(|hash| (hash.algorithm, hash.value))
                        .collect(),
                    pieces: set
                        .pieces
                        .map(|pieces| (pieces.algorithm, pieces.length, pieces.hashes)),
                })
                .collect()),
            Err(failure) => Err(refusal(failure)),
        }
    }

    /// Offers one URL for normalisation.
    pub async fn normalize(&self, url: &str) -> Result<Option<String>> {
        // Left as it is: the resolver refuses it with its own code (RD-120-66).
        if crate::foreign_address::carries_marker(url) {
            return Ok(None);
        }
        let mut store = self.runtime.store(None)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        match instance
            .rdownloader_plugin_intake()
            .call_normalize(&mut store, url)
            .await?
        {
            Ok(rewritten) => Ok(rewritten),
            Err(failure) => Err(refusal(failure)),
        }
    }
}

/// What a parser proposed, before the core has decided anything about it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntakeProposal {
    pub url: String,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    pub package_hint: Option<String>,
}

/// What a parser stated about one file's sources, before the core has checked any of it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSetProposal {
    /// The address the same parser proposed for this file through `parse`.
    pub primary_url: String,
    pub file_name: Option<String>,
    pub size: Option<u64>,
    /// `(url, priority, location)` in document order.
    pub sources: Vec<(String, Option<u32>, Option<String>)>,
    /// `(algorithm, value)` as the document spelled them.
    pub hashes: Vec<(String, String)>,
    /// `(algorithm, piece length, hashes)`.
    pub pieces: Option<(String, u64, Vec<String>)>,
}

/// A parser's refusal as an error: its stable code first, when it sent one, so the log names
/// it (RD-191-07, PLUG-16).
fn refusal(failure: crate::component::rdownloader::plugin::types::Failure) -> anyhow::Error {
    match failure.code {
        Some(code) => anyhow::anyhow!("{code}: {}", failure.message),
        None => anyhow::anyhow!("{}", failure.message),
    }
}
