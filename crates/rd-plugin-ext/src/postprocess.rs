//! Post-processing step plugins (RD-090-16).
//!
//! `rd-extract` owns the pipeline and knows nothing about WebAssembly — which is what keeps
//! its planner and its tests runnable without a runtime. This is the other half: the
//! implementation of the runner trait it takes, translating between a package on disk and a
//! plugin that may only ever name files, never locations.

use anyhow::Result;
use async_trait::async_trait;
use rd_extract::{PluginStepJob, PluginStepOutcome, PluginStepRunner, PluginStepWarning};
use rd_plugin_host::extension::{PostprocessPlugin, SourceState, StepOutcome};

use crate::PluginSet;

/// The installed post-processing steps, newest version of each, looked up by plugin id —
/// which is what a category stores.
pub type PluginSteps = PluginSet<PostprocessPlugin>;

/// What a step looks like to whoever is choosing one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepInfo {
    pub plugin_id: String,
    /// Default display name; the localised one comes from the plugin's own locale bundle.
    pub name: String,
    pub version: String,
}

impl PluginSteps {
    /// Every installed step, sorted by name so the list does not reshuffle itself.
    #[must_use]
    pub fn list(&self) -> Vec<StepInfo> {
        self.by_name()
            .into_iter()
            .map(|step| StepInfo {
                plugin_id: step.manifest.id.to_string(),
                name: step.manifest.name.clone(),
                version: step.manifest.version.clone(),
            })
            .collect()
    }
}

#[async_trait]
impl PluginStepRunner for PluginSteps {
    fn installed(&self, plugin_id: &str) -> bool {
        self.contains(plugin_id)
    }

    async fn run(&self, plugin_id: &str, job: PluginStepJob<'_>) -> Result<PluginStepOutcome> {
        let Some(step) = self.get(plugin_id) else {
            anyhow::bail!("no installed post-processing step with id {plugin_id}");
        };
        // The handle and the file list are all the plugin gets. The directory stays here.
        let source = SourceState::new(
            job.handle.to_owned(),
            job.directory.to_path_buf(),
            job.files.to_vec(),
        );
        Ok(
            match step
                .plugin
                .run(source, job.removed.to_vec(), job.checkpoint)
                .await?
            {
                StepOutcome::Complete { warnings, .. } => PluginStepOutcome::Complete {
                    warnings: warnings
                        .into_iter()
                        .map(|warning| PluginStepWarning {
                            code: warning.code,
                            params: warning.params.into_iter().collect(),
                            message: warning.message,
                        })
                        .collect(),
                },
                StepOutcome::Stopped { checkpoint } => PluginStepOutcome::Stopped { checkpoint },
                StepOutcome::Skipped => PluginStepOutcome::Skipped,
                StepOutcome::Failed { message } => PluginStepOutcome::Failed { message },
            },
        )
    }
}
