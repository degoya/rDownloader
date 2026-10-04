//! The lifecycle every verified upload step shares, whichever destination does the work
//! (audit 1.9.1, INTAKE-01/07).
//!
//! The object storage and the plugin destination paths were two copies of the same seventy
//! lines, and the copies drifted: the plugin one recorded its checkpoints under the
//! destination while the step had been planned under the configured remote. The planned row
//! stayed `Queued` for good, a second row appeared beside it, and `recover()` ran the whole
//! pipeline again on every start. One runner, given the label the step was planned under, is
//! what keeps the two from drifting again.

use std::path::Path;

use anyhow::Result;
use rd_core::{PostprocessKind, PostprocessStage, PostprocessState};

use crate::{
    rclone_job::UploadMode,
    steps::{Outcome, StepEnd, checkpoint, checkpoint_coded, checkpoint_outcome},
    storage_upload::{UploadProgress, UploadReport},
};

/// The destination is configured but nothing to run it is loaded.
pub(crate) const UPLOAD_UNAVAILABLE: &str = "postprocess.upload_unavailable";
/// Every file arrived and the destination confirmed it holds it.
pub(crate) const UPLOAD_VERIFIED: &str = "postprocess.upload_verified";
/// The same, and the confirmed files were removed locally (move mode).
pub(crate) const UPLOAD_VERIFIED_MOVED: &str = "postprocess.upload_verified_moved";
/// The destination or the transport refused; `detail` carries its words.
pub(crate) const UPLOAD_FAILED: &str = "postprocess.upload_failed";

/// One upload step: where it was planned, what it uploads and how.
pub(crate) struct UploadStep<'a> {
    pub owner: &'a str,
    /// The configured remote, exactly as the plan recorded it; every checkpoint names it.
    pub label: &'a str,
    /// What the queue shows as the step's current item: the destination itself, never the
    /// configured key with the plugin or profile id in front of it (audit 1.9.1, RA-IN-07).
    pub shown: &'a str,
    pub directory: &'a Path,
    pub mode: UploadMode,
    /// The English text for a destination nobody can serve right now.
    pub unavailable: &'a str,
}

/// Runs one upload step and says how it ended, as the rclone path does.
///
/// `upload` is `None` when the destination kind is configured but not loaded (the plugin was
/// removed, object storage is not wired); the step then fails with a stable code instead of an
/// upload that silently did not happen. Otherwise it is handed the progress reporter and does
/// the transfer; local files go only for what the destination confirmed (commit before delete).
pub(crate) async fn run<F, Fut>(
    inner: &crate::Inner,
    step: UploadStep<'_>,
    upload: Option<F>,
) -> Result<StepEnd>
where
    F: FnOnce(UploadProgress) -> Fut,
    Fut: std::future::Future<Output = Result<UploadReport>>,
{
    let Some(upload) = upload else {
        checkpoint_coded(
            inner,
            step.owner,
            PostprocessKind::Upload,
            step.label,
            PostprocessState::Failed,
            None,
            Outcome::new(UPLOAD_UNAVAILABLE, &[], step.unavailable),
        )
        .await?;
        return Ok(StepEnd::Failed);
    };
    crate::steps::stage(
        inner,
        step.owner,
        PostprocessStage::Uploading,
        Some(step.shown.to_owned()),
    )
    .await?;
    checkpoint(
        inner,
        step.owner,
        PostprocessKind::Upload,
        step.label,
        PostprocessState::Running,
        None,
        None,
    )
    .await?;
    let (progress, written) =
        crate::storage_upload::reporter(inner, step.owner, step.label, step.shown);
    let report = upload(progress).await;
    // The upload owned the only other handle on the sender, so it is closed by now and this
    // waits for the last number to be written rather than for anything to happen.
    let _ = written.await;
    let (state, outcome, end) = match report {
        Ok(UploadReport::Verified { files: uploaded }) => {
            let count = uploaded.len();
            let outcome = if step.mode == UploadMode::Move {
                // Only here, and only for the files the destination confirmed it holds: a
                // server that answered 201 and stored nothing would otherwise take the only
                // copy with it.
                let removed = crate::storage_upload::remove_local(step.directory, &uploaded).await;
                Outcome::new(
                    UPLOAD_VERIFIED_MOVED,
                    &[
                        ("count", count.to_string()),
                        ("removed", removed.to_string()),
                    ],
                    format!("{count} files uploaded and verified, {removed} removed locally"),
                )
            } else {
                Outcome::new(
                    UPLOAD_VERIFIED,
                    &[("count", count.to_string())],
                    format!("{count} files uploaded and verified"),
                )
            };
            (PostprocessState::Completed, Some(outcome), StepEnd::Done)
        }
        // Not a failure: what arrived stays, and the next run continues. The step goes back to
        // queued rather than being marked done on a job that is not, and the package is not
        // done either (RA-IN-01).
        Ok(UploadReport::Stopped) => (PostprocessState::Queued, None, StepEnd::Stopped),
        Ok(UploadReport::Failed { message }) => (
            PostprocessState::Failed,
            Some(Outcome::detailed(UPLOAD_FAILED, message)),
            StepEnd::Failed,
        ),
        Err(error) => (
            PostprocessState::Failed,
            Some(Outcome::detailed(UPLOAD_FAILED, format!("{error:#}"))),
            StepEnd::Failed,
        ),
    };
    checkpoint_outcome(
        inner,
        step.owner,
        PostprocessKind::Upload,
        step.label,
        state,
        None,
        outcome,
    )
    .await?;
    Ok(end)
}
