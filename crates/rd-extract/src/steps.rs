//! Small helpers shared by the pipeline stages: checkpoints, progress and step lookup.

use std::path::Path;

use anyhow::{Context, Result};
use rd_core::{PostprocessKind, PostprocessStage, PostprocessState, PostprocessStep};

use crate::Inner;

pub(crate) const MESSAGE_LIMIT: usize = 2_000;

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
