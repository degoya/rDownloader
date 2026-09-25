//! PAR2 verification/repair for every main index of a Usenet package.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::{PostprocessKind, PostprocessState, PostprocessStep};
use rd_postprocess::{Par2Error, is_main_par2, par2_set, verify_set};

use crate::{
    Inner,
    steps::{Outcome, checkpoint, checkpoint_coded, find_step, path_string, truncate},
};

/// Stable code for a recovery set that cannot close the gap it measured.
pub(crate) const NOT_ENOUGH_BLOCKS: &str = "postprocess.par2_not_enough_blocks";

/// One index that measured a gap it cannot close from what is on disk.
///
/// Carried out of the stage rather than acted on inside it: whether the gap is fatal depends
/// on recovery volumes that were never downloaded, and those are a queue matter, not a PAR2
/// one (RD-107-04).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BlockShortfall {
    /// The main index that reported the gap.
    pub index: PathBuf,
    /// Recovery blocks the repair is missing.
    pub needed: u32,
    /// Recovery blocks the set already offers on disk.
    pub available: u32,
}

/// What the PAR2 stage found out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Par2Outcome {
    /// Every index verified or repaired successfully.
    pub ok: bool,
    /// At least one index produced a verdict about the payload.
    ///
    /// A package with no index at all, or one whose every index turned out to be unreadable,
    /// has *not* been told anything about its files — that is what sends the substitute
    /// checks (SFV, RAR test) in, instead of treating silence as an answer (RD-104-04).
    pub answered: bool,
    /// The indexes that are short of recovery blocks, with how short they are.
    pub shortfalls: Vec<BlockShortfall>,
}

/// Verifies and repairs every main index of a package.
pub(crate) async fn run(
    inner: &Inner,
    owner: &str,
    steps: &[PostprocessStep],
    files: &[PathBuf],
    directory: &Path,
) -> Result<Par2Outcome> {
    let mut ok = true;
    let mut answered = false;
    let mut shortfalls = Vec::new();
    for index in files.iter().filter(|path| is_main_par2(path)) {
        let source = path_string(index)?;
        if find_step(steps, PostprocessKind::Par2, &source)
            .is_some_and(|step| step.state == PostprocessState::Completed)
        {
            // A previous run already verified this index; that verdict still stands.
            answered = true;
            continue;
        }
        crate::steps::stage(
            inner,
            owner,
            rd_core::PostprocessStage::Repairing,
            index.file_name().map(|n| n.to_string_lossy().into_owned()),
        )
        .await?;
        checkpoint(
            inner,
            owner,
            PostprocessKind::Par2,
            &source,
            PostprocessState::Running,
            None,
            None,
        )
        .await?;
        // Every volume of a set describes the same files, so a corrupt main index is a
        // reason to ask a sibling rather than to give up on the package (RD-104-04).
        let candidates = par2_set(index, files);
        match verify_set(&candidates, directory).await {
            Ok(report) => {
                answered = true;
                checkpoint(
                    inner,
                    owner,
                    PostprocessKind::Par2,
                    &source,
                    PostprocessState::Completed,
                    None,
                    Some(format!(
                        "repaired={} damaged={} missing={}",
                        report.repaired, report.damaged_files, report.missing_files
                    )),
                )
                .await?;
            }
            Err(error) => {
                ok = false;
                // "Not enough blocks" and "repair failed" are verdicts; an index nobody in
                // the set could read is not, and the substitute checks take over.
                answered = answered || !error.is_index_unreadable();
                if let Par2Error::NotEnoughBlocks { needed, available } = &error {
                    shortfalls.push(BlockShortfall {
                        index: index.clone(),
                        needed: *needed,
                        available: *available,
                    });
                    // A gap is the one outcome a person can be told something useful about,
                    // in their own language and with the two numbers that decide it.
                    checkpoint_coded(
                        inner,
                        owner,
                        PostprocessKind::Par2,
                        &source,
                        PostprocessState::Failed,
                        None,
                        Outcome {
                            code: NOT_ENOUGH_BLOCKS,
                            params: [
                                ("needed".to_owned(), needed.to_string()),
                                ("available".to_owned(), available.to_string()),
                            ]
                            .into_iter()
                            .collect(),
                            message: Some(truncate(error.to_string())),
                        },
                    )
                    .await?;
                    continue;
                }
                checkpoint(
                    inner,
                    owner,
                    PostprocessKind::Par2,
                    &source,
                    PostprocessState::Failed,
                    None,
                    Some(truncate(par2_message(&error, candidates.len()))),
                )
                .await?;
            }
        }
    }
    Ok(Par2Outcome {
        ok,
        answered,
        shortfalls,
    })
}

/// Failure text for a step row, naming how many members of the set were tried.
fn par2_message(error: &Par2Error, tried: usize) -> String {
    match error {
        Par2Error::IndexUnreadable(_) => {
            format!("{error}; none of the {tried} file(s) of this set could be read as an index")
        }
        _ => error.to_string(),
    }
}

/// Removes the PAR2 recovery set of every main index, once unpacking has succeeded.
///
/// Deliberately not part of [`run`]. PAR2 verification happens before extraction, so deleting
/// the recovery data there would leave a package whose unpack then failed — wrong password, a
/// missing tool — with nothing left to repair from and no second attempt possible. Here the
/// data has already done its job.
pub(crate) async fn delete_sets(
    inner: &Inner,
    owner: &str,
    steps: &[PostprocessStep],
    files: &[PathBuf],
) -> Result<()> {
    for index in files.iter().filter(|path| is_main_par2(path)) {
        let source = path_string(index)?;
        if find_step(steps, PostprocessKind::DeletePar2, &source)
            .is_some_and(|step| step.state == PostprocessState::Completed)
        {
            continue;
        }
        crate::steps::stage(inner, owner, rd_core::PostprocessStage::DeletingPar2, None).await?;
        checkpoint(
            inner,
            owner,
            PostprocessKind::DeletePar2,
            &source,
            PostprocessState::Running,
            None,
            None,
        )
        .await?;
        let set = rd_postprocess::par2_set(index, files);
        let mut removed = 0_usize;
        for path in &set {
            match tokio::fs::remove_file(path).await {
                Ok(()) => removed += 1,
                // Already gone is the outcome we wanted, not a failure.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    checkpoint(
                        inner,
                        owner,
                        PostprocessKind::DeletePar2,
                        &source,
                        PostprocessState::Failed,
                        None,
                        Some(truncate(format!("{}: {error}", path.display()))),
                    )
                    .await?;
                    return Ok(());
                }
            }
        }
        checkpoint(
            inner,
            owner,
            PostprocessKind::DeletePar2,
            &source,
            PostprocessState::Completed,
            None,
            Some(format!("removed={removed}")),
        )
        .await?;
    }
    Ok(())
}
