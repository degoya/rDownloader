//! Post-processing steps (RD-090-16): one step of the pipeline, resumable.

use std::sync::Arc;

use anyhow::Result;
use rd_plugin_api::ResolverHost;

use super::{ExtensionRuntime, SourceState, bindings::postprocess};
use crate::{PluginManifest, runtime::PluginStoreState};

/// How a post-processing step ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepOutcome {
    Complete { checkpoint: Option<Vec<u8>> },
    Stopped { checkpoint: Vec<u8> },
    Skipped,
    Failed { message: String },
}

/// A compiled post-processing step.
pub struct PostprocessPlugin {
    runtime: ExtensionRuntime,
    pre: postprocess::PostprocessPluginPre<PluginStoreState>,
}

impl PostprocessPlugin {
    pub fn new(
        manifest: PluginManifest,
        component_bytes: &[u8],
        host: Option<Arc<dyn ResolverHost>>,
    ) -> Result<Self> {
        let (runtime, pre) = ExtensionRuntime::build(manifest, component_bytes, host)?;
        Ok(Self {
            runtime,
            pre: postprocess::PostprocessPluginPre::new(pre)?,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        self.runtime.manifest()
    }

    /// Runs the step against one package.
    ///
    /// The package is reached through [`SourceState`]: the plugin is given a handle and the
    /// list of files it may read, never a path.
    pub async fn run(
        &self,
        source: SourceState,
        checkpoint: Option<Vec<u8>>,
    ) -> Result<StepOutcome> {
        let handle = source.handle().to_owned();
        let files = source.files().to_vec();
        let mut store = self.runtime.source_store(None, None, None, source)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let input = postprocess::exports::rdownloader::plugin::postprocess::StepInput {
            handle,
            files,
            checkpoint,
        };
        let end = instance
            .rdownloader_plugin_postprocess()
            .call_run(&mut store, &input)
            .await?;
        use postprocess::exports::rdownloader::plugin::postprocess::StepEnd;
        Ok(match end {
            StepEnd::Complete(checkpoint) => StepOutcome::Complete { checkpoint },
            StepEnd::Stopped(checkpoint) => StepOutcome::Stopped { checkpoint },
            StepEnd::Skipped => StepOutcome::Skipped,
            StepEnd::Failed(failure) => StepOutcome::Failed {
                message: failure.message,
            },
        })
    }
}
