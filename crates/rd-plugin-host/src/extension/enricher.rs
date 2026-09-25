//! Metadata enrichers (RD-090-14): additional fields for a link the core already knows.

use std::sync::Arc;

use anyhow::Result;
use rd_plugin_api::ResolverHost;

use super::{ExtensionRuntime, bindings::enricher};
use crate::{PluginManifest, runtime::PluginStoreState};

/// A compiled metadata enricher.
pub struct MetadataEnricher {
    runtime: ExtensionRuntime,
    pre: enricher::EnricherPluginPre<PluginStoreState>,
}

impl MetadataEnricher {
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: enricher::EnricherPluginPre::new(pre)?,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// Asks for additional fields. The caller decides which of them survive.
    pub async fn enrich(
        &self,
        url: &str,
        file_name: Option<&str>,
        known: Option<&str>,
    ) -> Result<Vec<(String, String)>> {
        let mut store = self.runtime.store(None)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let subject = enricher::exports::rdownloader::plugin::enricher::EnrichSubject {
            url: url.to_owned(),
            file_name: file_name.map(str::to_owned),
            known: known.map(str::to_owned),
        };
        let fields = instance
            .rdownloader_plugin_enricher()
            .call_enrich(&mut store, &subject)
            .await??;
        Ok(fields
            .into_iter()
            .map(|field| (field.name, field.value))
            .collect())
    }
}
