//! Post-processing steps contributed by plugins (RD-090-16).
//!
//! This crate owns the pipeline; it does not own a WebAssembly runtime and should not learn
//! about one. A step therefore arrives as a trait the service implements, the same way the
//! scheduler takes its external runners — which is also what keeps the planner and this
//! crate's tests runnable without wasmtime.

use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{PostprocessKind, PostprocessStage, PostprocessState};

use crate::{Inner, steps};

/// One package, as a plugin step is allowed to see it.
pub struct PluginStepJob<'a> {
    /// Package-scoped handle. Not a path: the plugin names files, never locations.
    pub handle: &'a str,
    pub directory: &'a Path,
    /// File names relative to the package directory, the only ones a step may read.
    pub files: &'a [String],
    /// What this step wrote when it last stopped.
    pub checkpoint: Option<Vec<u8>>,
}

/// How a plugin step ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginStepOutcome {
    Complete,
    /// Interrupted; the payload resumes it on the next attempt.
    Stopped {
        checkpoint: Vec<u8>,
    },
    /// Nothing to do for this package — not a failure, and not something to report as one.
    Skipped,
    Failed {
        message: String,
    },
}

/// The installed post-processing plugins, as this crate needs to see them.
#[async_trait]
pub trait PluginStepRunner: Send + Sync {
    /// Whether a plugin id names an installed step.
    ///
    /// Asked while planning, so a step that cannot run is not scheduled at all: a queued row
    /// nothing will ever pick up is worse than one that was never created.
    fn installed(&self, plugin_id: &str) -> bool;

    /// Runs one step against one package.
    async fn run(&self, plugin_id: &str, job: PluginStepJob<'_>) -> Result<PluginStepOutcome>;
}

/// Runs the plugin steps of one package, in the order they were planned.
///
/// Returns `false` if one of them failed, which the caller treats exactly as a failed unpack:
/// the package is not finished, and the reason is on the step.
pub(crate) async fn run(
    inner: &Inner,
    owner: &str,
    planned: &[String],
    directory: &Path,
    files: &[String],
) -> Result<bool> {
    let Some(runner) = inner.plugin_steps.as_ref() else {
        return Ok(true);
    };
    for plugin_id in planned {
        let existing = inner
            .database
            .list_postprocess_steps(owner)
            .await?
            .into_iter()
            .find(|step| {
                step.kind == PostprocessKind::PluginStep && &step.source_path == plugin_id
            });
        // A step that already finished stays finished: a package re-entering the pipeline
        // after a restart must not run somebody's checksum verification a second time.
        if existing
            .as_ref()
            .is_some_and(|step| step.state == PostprocessState::Completed)
        {
            continue;
        }
        let checkpoint = existing.and_then(|step| step.checkpoint);
        steps::stage(
            inner,
            owner,
            PostprocessStage::PluginStep,
            Some(plugin_id.clone()),
        )
        .await?;
        steps::checkpoint(
            inner,
            owner,
            PostprocessKind::PluginStep,
            plugin_id,
            PostprocessState::Running,
            None,
            None,
        )
        .await?;
        let outcome = runner
            .run(
                plugin_id,
                PluginStepJob {
                    handle: owner,
                    directory,
                    files,
                    checkpoint,
                },
            )
            .await;
        let (state, message, keep) = match outcome {
            Ok(PluginStepOutcome::Complete) => (PostprocessState::Completed, None, None),
            Ok(PluginStepOutcome::Skipped) => (PostprocessState::Skipped, None, None),
            // Left queued rather than failed: the step is unfinished, and the checkpoint is
            // what lets the next run continue instead of starting the package over.
            Ok(PluginStepOutcome::Stopped { checkpoint }) => {
                (PostprocessState::Queued, None, Some(checkpoint))
            }
            Ok(PluginStepOutcome::Failed { message }) => {
                (PostprocessState::Failed, Some(message), None)
            }
            // A plugin that traps, runs out of fuel or cannot be built is a failure of that
            // step, never of the package's other steps.
            Err(error) => (PostprocessState::Failed, Some(error.to_string()), None),
        };
        let failed = state == PostprocessState::Failed;
        inner
            .database
            .checkpoint_postprocess_with(
                owner.to_owned(),
                PostprocessKind::PluginStep,
                plugin_id.clone(),
                state,
                None,
                message.map(steps::truncate),
                keep,
            )
            .await?;
        if failed {
            return Ok(false);
        }
    }
    Ok(true)
}
