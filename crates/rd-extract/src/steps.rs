//! Small helpers shared by the pipeline stages: checkpoints, progress and step lookup.

use std::path::Path;

use anyhow::{Context, Result};
use rd_core::{PostprocessKind, PostprocessStage, PostprocessState, PostprocessStep};

use crate::Inner;

pub(crate) const MESSAGE_LIMIT: usize = 2_000;

/// How a step that a service stop can interrupt ended.
///
/// Three states, not a `bool`: a stop is neither a success nor a failure. Reported as success
/// it let the package be marked completed with its upload still queued, and the restart then
/// ran the whole pipeline — the user script included — a second time (audit 1.9.1, RA-IN-01).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StepEnd {
    Done,
    Failed,
    /// The service is stopping; the step is queued again and the package stays in
    /// post-processing for `recover()` to resume.
    Stopped,
}

pub(crate) async fn checkpoint(
    inner: &Inner,
    owner: &str,
    kind: PostprocessKind,
    source: &str,
    state: PostprocessState,
    output: Option<String>,
    message: Option<String>,
) -> Result<()> {
    inner
        .database
        .checkpoint_postprocess(
            owner.to_owned(),
            kind,
            source.to_owned(),
            state,
            output,
            message.map(|text| text.chars().take(MESSAGE_LIMIT).collect()),
        )
        .await
}

/// What a step has to say for itself: the stable code the interface translates, the parameters
/// that fill it, and the English text.
///
/// The three travel together because they are one outcome, not three arguments. `message` stays
/// the English text: a code the interface does not know still has to say something, and that
/// fallback is what keeps a new code from showing up as a blank line.
pub(crate) struct Outcome<'a> {
    pub(crate) code: &'a str,
    pub(crate) params: rd_core::MessageParams,
    pub(crate) message: Option<String>,
}

impl<'a> Outcome<'a> {
    /// An outcome from its code, its parameters as pairs and the English text.
    pub(crate) fn new(code: &'a str, params: &[(&str, String)], message: impl AsRef<str>) -> Self {
        Self {
            code,
            params: params
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
            message: Some(truncate(message)),
        }
    }

    /// An outcome whose `detail` parameter carries the tool's, the script's or the
    /// destination's own words — the part a code cannot know in advance.
    pub(crate) fn detailed(code: &'a str, detail: impl AsRef<str>) -> Self {
        let detail = truncate(detail);
        Self::new(code, &[("detail", detail.clone())], detail)
    }
}

/// The stable codes of the steps' own outcomes, looked up in `web/src/locales/*/server.json`
/// under `codes` like the extraction codes (audit 1.9.1, INTAKE-09: eight step kinds stored
/// English text only, which the interface could not translate).
pub(crate) mod codes {
    pub(crate) const UNPACK_COMPLETED: &str = "postprocess.unpack_completed";
    pub(crate) const ARCHIVES_REMOVED: &str = "postprocess.archives_removed";
    pub(crate) const PAR2_REPAIRED: &str = "postprocess.par2_repaired";
    pub(crate) const PAR2_FAILED: &str = "postprocess.par2_failed";
    pub(crate) const PAR2_REMOVED: &str = "postprocess.par2_removed";
    pub(crate) const DELETE_FAILED: &str = "postprocess.delete_failed";
    pub(crate) const SFV_VERIFIED: &str = "postprocess.sfv_verified";
    pub(crate) const SFV_MISMATCH: &str = "postprocess.sfv_mismatch";
    pub(crate) const SFV_FAILED: &str = "postprocess.sfv_failed";
    pub(crate) const RAR_TEST_PASSED: &str = "postprocess.rar_test_passed";
    pub(crate) const RAR_TEST_SKIPPED_PAR2: &str = "postprocess.rar_test_skipped_par2";
    pub(crate) const RAR_TEST_SKIPPED_SFV: &str = "postprocess.rar_test_skipped_sfv";
    pub(crate) const RAR_TEST_SKIPPED_OUTDATED: &str = "postprocess.rar_test_skipped_outdated";
    pub(crate) const RAR_TEST_SKIPPED_NO_TOOL: &str = "postprocess.rar_test_skipped_no_tool";
    pub(crate) const CLEANUP_DONE: &str = "postprocess.cleanup_done";
    pub(crate) const CLEANUP_FAILED: &str = "postprocess.cleanup_failed";
    pub(crate) const REMUX_FAILED: &str = "postprocess.remux_failed";
    pub(crate) const SCRIPT_SUCCEEDED: &str = "postprocess.script_succeeded";
    pub(crate) const SCRIPT_FAILED: &str = "postprocess.script_failed";
    pub(crate) const PLUGIN_STEP_FAILED: &str = "postprocess.plugin_step_failed";
    pub(crate) const UPLOAD_STALLED: &str = "postprocess.upload_stalled";
    /// A set unpacked while its package downloaded, moved into place (RD-1100-07).
    pub(crate) const UNPACK_COMPLETED_DIRECT: &str = "postprocess.unpack_completed_direct";
    /// The RAR test was not needed: the direct unpack checked every file (RD-1100-07).
    pub(crate) const RAR_TEST_SKIPPED_DIRECT: &str = "postprocess.rar_test_skipped_direct";
    pub(crate) const SORT_DONE: &str = "postprocess.sort_done";
    pub(crate) const SORT_FAILED: &str = "postprocess.sort_failed";
    pub(crate) const SORT_SKIPPED: &str = "postprocess.sort_skipped";
    /// A folder named like the package stayed: a name inside it is taken (RD-1140-01).
    pub(crate) const UNWRAP_CONFLICT: &str = "postprocess.unwrap_conflict";
    /// A folder named like the package could not be dissolved (RD-1140-01).
    pub(crate) const UNWRAP_FAILED: &str = "postprocess.unwrap_failed";
}

/// The same as `checkpoint`, with a translatable outcome instead of a bare message.
///
/// `output` is the same field `checkpoint` writes — the folder an unpack produced. It exists
/// here because the extraction steps need both, and having to choose between the output path
/// and a translatable code was why they smuggled their code into the text (RD-108-08).
pub(crate) async fn checkpoint_coded(
    inner: &Inner,
    owner: &str,
    kind: PostprocessKind,
    source: &str,
    state: PostprocessState,
    output: Option<String>,
    outcome: Outcome<'_>,
) -> Result<()> {
    inner
        .database
        .checkpoint_postprocess_coded(
            owner.to_owned(),
            kind,
            source.to_owned(),
            state,
            output,
            outcome
                .message
                .map(|text| text.chars().take(MESSAGE_LIMIT).collect()),
            Some(outcome.code.to_owned()),
            outcome.params,
        )
        .await
}

/// `checkpoint_coded` when the step has something to say, `checkpoint` without a message when
/// it has not (a stop that leaves the step queued, a success that needs no words).
pub(crate) async fn checkpoint_outcome(
    inner: &Inner,
    owner: &str,
    kind: PostprocessKind,
    source: &str,
    state: PostprocessState,
    output: Option<String>,
    outcome: Option<Outcome<'_>>,
) -> Result<()> {
    match outcome {
        Some(outcome) => checkpoint_coded(inner, owner, kind, source, state, output, outcome).await,
        None => checkpoint(inner, owner, kind, source, state, output, None).await,
    }
}

/// What an extraction failure has to say: its stable code, the tool's own words as the
/// `detail` parameter the catalogues interpolate, and the English text as the fallback.
///
/// One place rather than four call sites that each decide how to phrase the same thing.
pub(crate) fn extraction_outcome(error: &rd_postprocess::ExtractionError) -> Outcome<'static> {
    Outcome {
        code: error.code(),
        params: error
            .detail()
            .map(|detail| {
                [("detail".to_owned(), truncate(detail))]
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default(),
        message: Some(truncate(error.message())),
    }
}

/// Publishes the package stage (and optional percent/current) for the queue view.
pub(crate) async fn stage(
    inner: &Inner,
    owner: &str,
    stage: PostprocessStage,
    current: Option<String>,
) -> Result<()> {
    let id: rd_core::PackageId = owner.parse().context("owner is not a package id")?;
    inner
        .database
        .set_package_state(
            id,
            rd_core::PackageState::Postprocessing,
            Some(stage),
            None,
            current,
        )
        .await
}

pub(crate) fn find_step<'a>(
    steps: &'a [PostprocessStep],
    kind: PostprocessKind,
    source: &str,
) -> Option<&'a PostprocessStep> {
    steps
        .iter()
        .find(|step| step.kind == kind && step.source_path == source)
}

pub(crate) fn path_string(path: &Path) -> Result<String> {
    path.to_str()
        .context("postprocessing path is not UTF-8")
        .map(str::to_owned)
}

/// Truncates free text for step messages.
pub(crate) fn truncate(text: impl AsRef<str>) -> String {
    text.as_ref().chars().take(MESSAGE_LIMIT).collect()
}

/// Forwards progress samples to the database at most every 750 ms (last sample always).
pub(crate) async fn drain_progress(
    database: rd_db::Database,
    owner: String,
    kind: PostprocessKind,
    source: String,
    stage: PostprocessStage,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<rd_postprocess::ExtractProgress>,
) {
    const INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);
    let mut last_write: Option<tokio::time::Instant> = None;
    let mut latest: Option<rd_postprocess::ExtractProgress> = None;
    let mut written_percent = None;
    loop {
        let sample = match latest.take() {
            Some(pending) => {
                let wait = last_write.map_or(std::time::Duration::ZERO, |at| {
                    INTERVAL.saturating_sub(at.elapsed())
                });
                tokio::select! {
                    () = tokio::time::sleep(wait) => pending,
                    next = receiver.recv() => match next {
                        Some(next) => { latest = Some(next); continue; }
                        None => pending,
                    },
                }
            }
            None => match receiver.recv().await {
                Some(sample) => {
                    latest = Some(sample);
                    continue;
                }
                None => break,
            },
        };
        if sample.percent != written_percent || sample.current.is_some() {
            let _ = database
                .postprocess_progress(
                    owner.clone(),
                    kind,
                    source.clone(),
                    stage,
                    sample.percent,
                    sample.current.clone(),
                )
                .await;
            written_percent = sample.percent;
            last_write = Some(tokio::time::Instant::now());
        }
    }
}
