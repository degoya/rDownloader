//! Post-processing step plugins (RD-090-16).
//!
//! `rd-extract` owns the pipeline and knows nothing about WebAssembly — which is what keeps
//! its planner and its tests runnable without a runtime. This is the other half: the
//! implementation of the runner trait it takes, translating between a package on disk and a
//! plugin that may only ever name files, never locations.

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use rd_extract::{PluginStepJob, PluginStepOutcome, PluginStepRunner};
use rd_plugin_api::ResolverHost;
use rd_plugin_host::{
    PluginInstaller, PluginManifest, PluginType, PluginTypeRegistry,
    extension::{PostprocessPlugin, SourceState, StepOutcome},
};

/// The installed post-processing steps, newest version of each.
pub struct PluginSteps {
    /// Keyed by plugin id, which is what a category stores.
    plugins: HashMap<String, Step>,
}

struct Step {
    manifest: PluginManifest,
    plugin: PostprocessPlugin,
}

/// What a step looks like to whoever is choosing one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepInfo {
    pub plugin_id: String,
    /// Default display name; the localised one comes from the plugin's own locale bundle.
    pub name: String,
    pub version: String,
}

impl PluginSteps {
    /// Loads every installed post-processing step, skipping any that fails to build.
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
    ///
    /// Loading a registry re-verifies and compiles every installed package, so the one `load`
    /// builds for itself is only worth it for a caller that loads a single adapter. Everything
    /// started together passes one registry through all of them.
    #[must_use]
    pub fn from_registry(
        registry: &PluginTypeRegistry,
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Self {
        let loaded = registry.instantiate(&PluginType::Postprocess, |package| {
            PostprocessPlugin::new(package.manifest.clone(), &package.component, host.clone()).map(
                |plugin| Step {
                    manifest: package.manifest.clone(),
                    plugin,
                },
            )
        });
        // Newest version of each plugin comes first, so the first entry for an id wins.
        let mut plugins = HashMap::new();
        for step in loaded {
            plugins.entry(step.manifest.id.to_string()).or_insert(step);
        }
        Self { plugins }
    }

    /// An empty set, for a service running without plugins.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Every installed step, sorted by name so the list does not reshuffle itself.
    #[must_use]
    pub fn list(&self) -> Vec<StepInfo> {
        let mut steps: Vec<StepInfo> = self
            .plugins
            .values()
            .map(|step| StepInfo {
                plugin_id: step.manifest.id.to_string(),
                name: step.manifest.name.clone(),
                version: step.manifest.version.clone(),
            })
            .collect();
        steps.sort_by(|left, right| left.name.cmp(&right.name));
        steps
    }
}

#[async_trait]
impl PluginStepRunner for PluginSteps {
    fn installed(&self, plugin_id: &str) -> bool {
        self.plugins.contains_key(plugin_id)
    }

    async fn run(&self, plugin_id: &str, job: PluginStepJob<'_>) -> Result<PluginStepOutcome> {
        let Some(step) = self.plugins.get(plugin_id) else {
            anyhow::bail!("no installed post-processing step with id {plugin_id}");
        };
        // The handle and the file list are all the plugin gets. The directory stays here.
        let source = SourceState::new(
            job.handle.to_owned(),
            job.directory.to_path_buf(),
            job.files.to_vec(),
        );
        Ok(match step.plugin.run(source, job.checkpoint).await? {
            StepOutcome::Complete { .. } => PluginStepOutcome::Complete,
            StepOutcome::Stopped { checkpoint } => PluginStepOutcome::Stopped { checkpoint },
            StepOutcome::Skipped => PluginStepOutcome::Skipped,
            StepOutcome::Failed { message } => PluginStepOutcome::Failed { message },
        })
    }
}
