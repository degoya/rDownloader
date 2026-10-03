//! Post-processing steps (RD-090-16): one step of the pipeline, resumable.

use std::sync::Arc;

use anyhow::Result;
use rd_plugin_api::ResolverHost;

use super::{ExtensionRuntime, SourceState, bindings::postprocess};
use crate::{PluginManifest, runtime::PluginStoreState};

/// Most warnings one step may hand back; the rest are dropped. A step names what it could not
/// check, it does not write a report.
pub const MAX_STEP_WARNINGS: usize = 16;

/// How a post-processing step ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepOutcome {
    Complete {
        checkpoint: Option<Vec<u8>>,
        /// What the step wants shown although it passed (RD-190-06), at most
        /// [`MAX_STEP_WARNINGS`].
        warnings: Vec<StepWarning>,
    },
    Stopped {
        checkpoint: Vec<u8>,
    },
    Skipped,
    Failed {
        message: String,
    },
}

/// One warning of a step that passed: a code from the plugin's catalogue, its parameters and
/// the English text for a code no catalogue knows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepWarning {
    pub code: String,
    pub params: Vec<(String, String)>,
    pub message: String,
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
    /// list of files it may read, never a path. `removed` names, the same way, the files the
    /// pipeline removed before this step (RD-190-06); none of them can be read.
    pub async fn run(
        &self,
        source: SourceState,
        removed: Vec<String>,
        checkpoint: Option<Vec<u8>>,
    ) -> Result<StepOutcome> {
        let handle = source.handle().to_owned();
        let files = source.files().to_vec();
        let mut store = self.runtime.source_store(None, None, None, source)?;
        let instance = self.pre.instantiate_async(&mut store).await?;
        let input = postprocess::exports::rdownloader::plugin::postprocess::StepInput {
            handle,
            files,
            removed,
            checkpoint,
        };
        let end = instance
            .rdownloader_plugin_postprocess()
            .call_run(&mut store, &input)
            .await?;
        use postprocess::exports::rdownloader::plugin::postprocess::StepEnd;
        Ok(match end {
            StepEnd::Complete(complete) => StepOutcome::Complete {
                checkpoint: complete.checkpoint,
                warnings: complete
                    .warnings
                    .into_iter()
                    .take(MAX_STEP_WARNINGS)
                    .map(|part| StepWarning {
                        code: part.code,
                        params: part.params,
                        message: part.message,
                    })
                    .collect(),
            },
            StepEnd::Stopped(checkpoint) => StepOutcome::Stopped { checkpoint },
            StepEnd::Skipped => StepOutcome::Skipped,
            StepEnd::Failed(failure) => StepOutcome::Failed {
                message: failure.message,
            },
        })
    }
}
