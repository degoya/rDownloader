//! Post-processing steps contributed by plugins (RD-090-16).
//!
//! This crate owns the pipeline; it does not own a WebAssembly runtime and should not learn
//! about one. A step therefore arrives as a trait the service implements, the same way the
//! scheduler takes its external runners — which is also what keeps the planner and this
//! crate's tests runnable without wasmtime.

use std::collections::HashSet;
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
    /// File paths relative to the package directory, `/` between folders — the only files a
    /// step may read.
    pub files: &'a [String],
    /// Files the package had that the pipeline removed before the plugin steps ran, named like
    /// `files` (RD-190-06): unpacked volumes, PAR2 files, what the cleanup deleted.
    pub removed: &'a [String],
    /// What this step wrote when it last stopped.
    pub checkpoint: Option<Vec<u8>>,
}

/// One warning of a step that passed (RD-190-06): a code from the plugin's catalogue, its
/// parameters and the English text for a code no catalogue knows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginStepWarning {
    pub code: String,
    pub params: rd_core::MessageParams,
    pub message: String,
}

/// How a plugin step ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginStepOutcome {
    /// Passed; what the step still wants shown goes on the step, never as a failure.
    Complete {
        warnings: Vec<PluginStepWarning>,
    },
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
    removed: &[String],
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
                    removed,
                    checkpoint,
                },
            )
            .await;
        let (state, message, keep) = match outcome {
            Ok(PluginStepOutcome::Complete { warnings }) if !warnings.is_empty() => {
                record_warnings(inner, owner, plugin_id, warnings).await?;
                continue;
            }
            Ok(PluginStepOutcome::Complete { .. }) => (PostprocessState::Completed, None, None),
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

/// Records a step that passed with warnings: completed, with the warning on the step where the
/// interface shows it, and each one in the service log.
///
/// A step row holds one code. A single warning keeps its code and parameters, so the interface
/// translates it from the plugin's catalogue; several are recorded as their English texts in
/// one message, which says all of them rather than translating only the first.
async fn record_warnings(
    inner: &Inner,
    owner: &str,
    plugin_id: &str,
    warnings: Vec<PluginStepWarning>,
) -> Result<()> {
    for warning in &warnings {
        tracing::warn!(
            package = owner,
            plugin_id,
            code = %warning.code,
            "{}",
            warning.message
        );
    }
    let message = steps::truncate(
        warnings
            .iter()
            .map(|warning| warning.message.as_str())
            .collect::<Vec<_>>()
            .join("; "),
    );
    let (code, params) = match warnings.as_slice() {
        [only] if !only.code.is_empty() => (Some(only.code.clone()), only.params.clone()),
        _ => (None, rd_core::MessageParams::new()),
    };
    inner
        .database
        .checkpoint_postprocess_coded(
            owner.to_owned(),
            PostprocessKind::PluginStep,
            plugin_id.to_owned(),
            PostprocessState::Completed,
            None,
            Some(message),
            code,
            params,
        )
        .await
}

/// The files the package had that are gone by the time the plugin steps run (RD-190-06),
/// named like `present`.
///
/// Three sources, because none of them is complete alone: what the folder held when this run
/// began, what the package downloaded (which still knows a volume an earlier, interrupted run
/// already unpacked and removed) and what the cleanup deleted out of the unpacked content.
/// An archive that came out of an archive and went again after its own unpack is in none of
/// them; a checksum list naming one leaves it unchecked, with a warning.
pub(crate) fn removed_files(
    present: &[String],
    before: &[String],
    downloaded: &[String],
    cleaned: &[String],
) -> Vec<String> {
    let present: HashSet<&str> = present.iter().map(String::as_str).collect();
    let mut removed: Vec<String> = before
        .iter()
        .chain(downloaded)
        .chain(cleaned)
        .filter(|name| !present.contains(name.as_str()))
        .cloned()
        .collect();
    removed.sort();
    removed.dedup();
    removed
}

/// A path relative to the package folder, `/` between folders whatever the platform writes.
pub(crate) fn relative_name(path: &Path) -> String {
    path.components()
        .filter_map(|part| match part {
            std::path::Component::Normal(name) => Some(name.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{relative_name, removed_files};

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn removed_is_everything_the_package_had_and_lost_once() {
        let present = names(&["Film/film.md5", "Film/film.mkv"]);
        // The folder at the start of this run, the downloads (one of them unpacked and removed
        // by an earlier, interrupted run) and the cleanup's own list overlap; each name once.
        let before = names(&["Film.part1.rar", "Film.part2.rar", "Film.par2"]);
        let downloaded = names(&["Film.part0.rar", "Film.part1.rar", "Film.part2.rar"]);
        let cleaned = names(&["Film/film.nfo"]);
        assert_eq!(
            removed_files(&present, &before, &downloaded, &cleaned),
            names(&[
                "Film.par2",
                "Film.part0.rar",
                "Film.part1.rar",
                "Film.part2.rar",
                "Film/film.nfo",
            ])
        );
    }

    #[test]
    fn a_file_that_is_still_there_is_not_removed() {
        let present = names(&["Film.zip", "Film/film.mkv"]);
        let downloaded = names(&["Film.zip"]);
        assert!(removed_files(&present, &present, &downloaded, &[]).is_empty());
    }

    #[test]
    fn a_relative_name_has_forward_slashes_and_no_dots() {
        assert_eq!(
            relative_name(&Path::new("Film").join("Extras").join("a.nfo")),
            "Film/Extras/a.nfo"
        );
        assert_eq!(relative_name(Path::new("./Film/a.nfo")), "Film/a.nfo");
    }
}
