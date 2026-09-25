//! CRC32 verification of every `.sfv` index found in a package.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rd_core::{PostprocessKind, PostprocessStage, PostprocessState, PostprocessStep};
use rd_postprocess::{SfvReport, verify_sfv};

use crate::{
    Inner,
    steps::{checkpoint, drain_progress, find_step, path_string, truncate},
};

/// Names listed in a failure message before it is cut short; the full count is always given.
const NAMED_FAILURES: usize = 10;

/// Returns `false` when at least one index reported a mismatch or a missing file.
pub(crate) async fn run(
    inner: &Inner,
    owner: &str,
    steps: &[PostprocessStep],
    indexes: &[PathBuf],
    directory: &Path,
) -> Result<bool> {
    let mut ok = true;
    for index in indexes {
        let source = path_string(index)?;
        if find_step(steps, PostprocessKind::Sfv, &source)
            .is_some_and(|step| step.state == PostprocessState::Completed)
        {
            continue;
        }
        crate::steps::stage(
            inner,
            owner,
            PostprocessStage::Verifying,
            index.file_name().map(|n| n.to_string_lossy().into_owned()),
        )
        .await?;
        checkpoint(
            inner,
            owner,
            PostprocessKind::Sfv,
            &source,
            PostprocessState::Running,
            None,
            None,
        )
        .await?;
        let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let drain = tokio::spawn(drain_progress(
            inner.database.clone(),
            owner.to_owned(),
            PostprocessKind::Sfv,
            source.clone(),
            PostprocessStage::Verifying,
            progress_rx,
        ));
        let result = verify_sfv(index.clone(), directory.to_owned(), Some(&progress_tx)).await;
        drop(progress_tx);
        let _ = drain.await;
        match result {
            Ok(report) if report.is_ok() => {
                checkpoint(
                    inner,
                    owner,
                    PostprocessKind::Sfv,
                    &source,
                    PostprocessState::Completed,
                    None,
                    Some(format!(
                        "checked={} skipped={}",
                        report.checked, report.skipped
                    )),
                )
                .await?;
            }
            Ok(report) => {
                ok = false;
                checkpoint(
                    inner,
                    owner,
                    PostprocessKind::Sfv,
                    &source,
                    PostprocessState::Failed,
                    None,
                    Some(truncate(failure_message(&report))),
                )
                .await?;
            }
            Err(error) => {
                ok = false;
                checkpoint(
                    inner,
                    owner,
                    PostprocessKind::Sfv,
                    &source,
                    PostprocessState::Failed,
                    None,
                    Some(truncate(error.to_string())),
                )
                .await?;
            }
        }
    }
    Ok(ok)
}

/// Counts first so a long index stays readable, then names the files that actually failed.
fn failure_message(report: &SfvReport) -> String {
    let mut text = format!(
        "checked={} mismatch={} missing={}",
        report.checked,
        report.mismatched.len(),
        report.missing.len()
    );
    if !report.mismatched.is_empty() {
        text.push_str(&format!(" mismatched: {}", names(&report.mismatched)));
    }
    if !report.missing.is_empty() {
        text.push_str(&format!(" absent: {}", names(&report.missing)));
    }
    text
}

fn names(values: &[String]) -> String {
    let listed = values
        .iter()
        .take(NAMED_FAILURES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > NAMED_FAILURES {
        format!("{listed}, … ({} more)", values.len() - NAMED_FAILURES)
    } else {
        listed
    }
}

#[cfg(test)]
mod tests {
    use rd_postprocess::SfvReport;

    use super::failure_message;

    #[test]
    fn a_failure_message_counts_first_and_then_names_the_files() {
        let report = SfvReport {
            checked: 3,
            mismatched: vec!["a.r00".to_owned(), "a.r01".to_owned()],
            missing: vec!["a.r02".to_owned()],
            skipped: 0,
        };
        assert_eq!(
            failure_message(&report),
            "checked=3 mismatch=2 missing=1 mismatched: a.r00, a.r01 absent: a.r02"
        );
    }

    #[test]
    fn a_long_failure_list_is_summarised_instead_of_printed_in_full() {
        let report = SfvReport {
            checked: 12,
            mismatched: (0..12).map(|index| format!("part{index}.rar")).collect(),
            missing: Vec::new(),
            skipped: 0,
        };
        let message = failure_message(&report);
        assert!(message.starts_with("checked=12 mismatch=12 missing=0"));
        assert!(message.ends_with("… (2 more)"));
        assert!(!message.contains("part10.rar"));
    }
}
