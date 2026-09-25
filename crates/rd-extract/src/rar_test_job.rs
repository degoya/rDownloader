//! Substitute integrity check: ask the RAR set itself whether it arrived intact.
//!
//! SABnzbd's `try_rar_check`, and it runs for the same reason (RD-104-04): with no PAR2 set
//! and no `.sfv` index, nothing has told the package whether its payload survived the wire,
//! and unpacking straight into a half-arrived archive turns a recoverable download into a
//! folder of rubbish. Reading the volumes and verifying their stored CRCs answers that
//! without writing anything.

use rd_core::{PostprocessKind, PostprocessStage, PostprocessState, PostprocessStep};
use rd_postprocess::{ArchiveSet, ExternalRarTool, ExtractionError, test_rar};

use anyhow::Result;

use crate::{
    Inner,
    steps::{checkpoint, checkpoint_coded, extraction_outcome, find_step, path_string},
};

/// The RAR sets of a package, in plan order.
pub(crate) fn rar_sets(sets: &[ArchiveSet]) -> Vec<&ArchiveSet> {
    sets.iter()
        .filter(|set| set.kind == rd_files::ArchiveKind::Rar)
        .collect()
}

/// Records every planned RAR test as skipped, with the reason it was not needed.
pub(crate) async fn skip(
    inner: &Inner,
    owner: &str,
    steps: &[PostprocessStep],
    sets: &[ArchiveSet],
    reason: &str,
) -> Result<()> {
    for set in rar_sets(sets) {
        let source = path_string(set.first())?;
        if find_step(steps, PostprocessKind::RarTest, &source)
            .is_some_and(|step| step.state == PostprocessState::Completed)
        {
            continue;
        }
        checkpoint(
            inner,
            owner,
            PostprocessKind::RarTest,
            &source,
            PostprocessState::Skipped,
            None,
            Some(reason.to_owned()),
        )
        .await?;
    }
    Ok(())
}

/// Tests every RAR set of the package. Returns `false` when one of them is damaged.
///
/// A missing RAR tool is not a failure: it means the question could not be asked, and
/// refusing to unpack because nothing was there to test would be the opposite of the point.
pub(crate) async fn run(
    inner: &Inner,
    owner: &str,
    steps: &[PostprocessStep],
    sets: &[ArchiveSet],
    tool: Option<&ExternalRarTool>,
    candidates: &[Option<String>],
) -> Result<bool> {
    let Some(tool) = tool else {
        skip(inner, owner, steps, sets, "no RAR tool available").await?;
        return Ok(true);
    };
    let mut ok = true;
    for set in rar_sets(sets) {
        let source = path_string(set.first())?;
        if find_step(steps, PostprocessKind::RarTest, &source)
            .is_some_and(|step| step.state == PostprocessState::Completed)
        {
            continue;
        }
        crate::steps::stage(
            inner,
            owner,
            PostprocessStage::Verifying,
            set.first()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned()),
        )
        .await?;
        checkpoint(
            inner,
            owner,
            PostprocessKind::RarTest,
            &source,
            PostprocessState::Running,
            None,
            None,
        )
        .await?;
        match test_set(tool, set, candidates).await {
            Ok(()) => {
                checkpoint(
                    inner,
                    owner,
                    PostprocessKind::RarTest,
                    &source,
                    PostprocessState::Completed,
                    None,
                    Some(format!("volumes={}", set.volumes.len())),
                )
                .await?;
            }
            // An archive nobody has the password for is not a damaged archive. Saying so
            // would block the unpack that is about to ask the user's password list anyway.
            Err(error) if error.is_password_problem() => {
                checkpoint_coded(
                    inner,
                    owner,
                    PostprocessKind::RarTest,
                    &source,
                    PostprocessState::Skipped,
                    None,
                    extraction_outcome(&error),
                )
                .await?;
            }
            Err(error) => {
                ok = false;
                checkpoint_coded(
                    inner,
                    owner,
                    PostprocessKind::RarTest,
                    &source,
                    PostprocessState::Failed,
                    None,
                    extraction_outcome(&error),
                )
                .await?;
            }
        }
    }
    Ok(ok)
}

/// Tries the known passwords in turn, the same order the unpack would.
async fn test_set(
    tool: &ExternalRarTool,
    set: &ArchiveSet,
    candidates: &[Option<String>],
) -> Result<(), ExtractionError> {
    let mut last = ExtractionError::PasswordRequired;
    let attempts: Vec<Option<String>> = if candidates.is_empty() {
        vec![None]
    } else {
        candidates.to_vec()
    };
    for password in attempts {
        match test_rar(tool, set.first(), password.as_deref()).await {
            Ok(()) => return Ok(()),
            Err(error) if error.is_password_problem() => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}
