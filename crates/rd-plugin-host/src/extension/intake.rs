//! Intake parsers (RD-090-12): text and URLs in, LinkGrabber candidates out.

use std::sync::Arc;

use anyhow::Result;
use rd_plugin_api::ResolverHost;

use super::{ExtensionRuntime, bindings::intake};
use crate::{PluginManifest, runtime::PluginStoreState};

/// A compiled intake parser, pinned to one installed manifest version.
pub struct IntakeParser {
    runtime: ExtensionRuntime,
    pre: intake::IntakePluginPre<PluginStoreState>,
}

impl IntakeParser {
    /// Compiles a verified package and links only what its manifest grants.
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: intake::IntakePluginPre::new(pre)?,
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
            Err(failure) => anyhow::bail!("{}", failure.message),
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
            Err(failure) => anyhow::bail!("{}", failure.message),
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
