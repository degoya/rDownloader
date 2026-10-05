//! Sorting and renaming series episodes and films by the category's templates (RD-1100-08).
//!
//! The last step of a package that succeeded: everything that works on the package as a whole —
//! the script, the upload — has seen it before its files move. What is recognised and where it
//! goes is planned by `rd_files::plan_sort`, the same code the category editor's preview runs;
//! this step moves the files, below the folder the package was put in and nowhere else.
//!
//! Built in rather than a post-processing plugin: a plugin step is handed a package handle and a
//! file list and may rename inside the package only — the host refuses a name with a separator —
//! while a sort creates folders beside the package, below the category's folder. That boundary
//! is the point of the plugin contract and stays as it is; the sort is checked here instead,
//! against the root on disk, and never through a link sitting in the library.
//!
//! A name taken at the target follows the package's collision policy, as a finished download
//! does (RD-150-01): `rename` moves beside it as `name (1)`, `skip` and `ask` leave the file in
//! the package (no prompt here: nothing waits on an answer at the end of the pipeline), `compare`
//! drops an identical copy and renames a different one, `overwrite` replaces it and is audited.

use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use rd_core::{
    AuditAction, AuditOutcome, CollisionPolicy, PostprocessKind, PostprocessStage,
    PostprocessState, SortTemplates,
};

use crate::{
    Inner,
    package_job::{Run, package_file_names},
    steps::{Outcome, StepEnd, checkpoint, checkpoint_coded, codes, truncate},
};

/// The step's `source`; there is one sort per package.
pub(crate) const SOURCE: &str = "sort";

/// What became of one file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Placed {
    /// Moved to its target, or beside it under a free name.
    Moved,
    /// An identical file was already there; the package's copy was dropped.
    Identical,
    /// The file it replaced is recorded in the audit log.
    Replaced,
    /// The policy kept it in the package.
    Left,
}

/// Sorts the package and records the step.
pub(crate) async fn run(run: &Run<'_>, templates: &SortTemplates) -> Result<StepEnd> {
    let inner = run.inner;
    crate::steps::stage(inner, &run.owner, PostprocessStage::Sorting, None).await?;
    checkpoint(
        inner,
        &run.owner,
        PostprocessKind::Sort,
        SOURCE,
        PostprocessState::Running,
        None,
        None,
    )
    .await?;
    // A package folder always sits directly below the folder its category resolved to
    // (`rd_files::package_directory`); that folder is the root nothing may leave.
    let Some(root) = run.directory.parent() else {
        return finish(run, 0, 0, Vec::new()).await;
    };
    let Ok(canonical_root) = dunce::canonicalize(root) else {
        return finish(
            run,
            0,
            0,
            vec![format!("{}: not reachable", root.display())],
        )
        .await;
    };
    let files: Vec<String> = package_file_names(&run.directory)
        .await
        .into_iter()
        .filter(|name| !is_sample(name))
        .collect();
    let plan = rd_files::plan_sort(root, &run.package.name, &files, templates);
    let policy = effective_policy(run).await?;
    let mut placed = 0_usize;
    let mut left = plan.unsorted.len() + plan.refused.len();
    let mut errors = Vec::new();
    let mut moved_from = Vec::new();
    for entry in &plan.moves {
        let from = run.directory.join(&entry.from);
        match place(&from, &entry.to, root, &canonical_root, policy).await {
            Ok(Placed::Left) => left += 1,
            Ok(outcome) => {
                placed += 1;
                moved_from.push(from);
                if outcome == Placed::Replaced {
                    audit_replaced(inner, run, &entry.to).await;
                }
                // A stop between two moves leaves some files placed and the rest in the
                // package; the next start sorts what is left and finds the placed ones gone.
                rd_core::failpoint!("postprocess.after_sort_move", || anyhow::anyhow!(
                    "crash point postprocess.after_sort_move"
                ));
            }
            Err(error) => errors.push(format!("{}: {error:#}", entry.from)),
        }
    }
    for (name, error) in &plan.refused {
        tracing::info!(package_id = %run.package_id, file = %name, %error, "sort template refused a name");
    }
    if let Ok(package) = dunce::canonicalize(&run.directory) {
        let moved: Vec<PathBuf> = moved_from
            .iter()
            .filter_map(|path| path.strip_prefix(&run.directory).ok())
            .map(|relative| package.join(relative))
            .collect();
        crate::cleanup_job::remove_emptied_folders(&package, &moved).await;
        // `remove_dir` refuses a folder that still holds anything: a package with files the
        // sort left behind keeps its folder, an emptied one does not linger in the library.
        if !moved.is_empty() {
            let _ = tokio::fs::remove_dir(&package).await;
        }
    }
    finish(run, placed, left, errors).await
}

/// Records the sort as skipped when the package failed before it: its files stay where they
/// are, so a retry finds them. A step something else already ended is left as it is.
pub(crate) async fn skip(inner: &Inner, owner: &str) -> Result<()> {
    let queued = inner
        .database
        .list_postprocess_steps(owner)
        .await?
        .into_iter()
        .any(|step| step.kind == PostprocessKind::Sort && step.state == PostprocessState::Queued);
    if !queued {
        return Ok(());
    }
    checkpoint_coded(
        inner,
        owner,
        PostprocessKind::Sort,
        SOURCE,
        PostprocessState::Skipped,
        None,
        Outcome::new(
            codes::SORT_SKIPPED,
            &[],
            "Not sorted: the package did not finish",
        ),
    )
    .await
}

async fn finish(run: &Run<'_>, placed: usize, left: usize, errors: Vec<String>) -> Result<StepEnd> {
    let counts = [("placed", placed.to_string()), ("left", left.to_string())];
    let (state, outcome, end) = if errors.is_empty() {
        (
            PostprocessState::Completed,
            Outcome::new(
                codes::SORT_DONE,
                &counts,
                format!("placed={placed} left={left}"),
            ),
            StepEnd::Done,
        )
    } else {
        let detail = truncate(errors.join("; "));
        (
            PostprocessState::Failed,
            Outcome::new(
                codes::SORT_FAILED,
                &[
                    counts[0].clone(),
                    counts[1].clone(),
                    ("detail", detail.clone()),
                ],
                format!("placed={placed} left={left} errors={detail}"),
            ),
            StepEnd::Failed,
        )
    };
    checkpoint_coded(
        run.inner,
        &run.owner,
        PostprocessKind::Sort,
        SOURCE,
        state,
        None,
        outcome,
    )
    .await?;
    Ok(end)
}

/// The package's own policy, else its category's, else the global one.
async fn effective_policy(run: &Run<'_>) -> Result<CollisionPolicy> {
    let levels = run
        .inner
        .database
        .collision_policy_levels(run.package_id)
        .await?;
    let global = run
        .inner
        .database
        .service_settings_or_default::<rd_core::StorageSettings>()
        .await?
        .storage_collision_policy;
    Ok(rd_core::effective_collision_policy(levels.package, levels.category, global).policy)
}

/// Moves one file to `to` under `policy`, into a folder created below the root one level at a
/// time: an existing level has to be a folder, not a link, so nothing — not even an empty folder —
/// is created outside the category's folder through a link that sits in the library.
async fn place(
    from: &Path,
    to: &Path,
    root: &Path,
    canonical_root: &Path,
    policy: CollisionPolicy,
) -> Result<Placed> {
    let (Some(folder), Some(name)) = (to.parent(), to.file_name()) else {
        anyhow::bail!("{} has no folder", to.display());
    };
    let relative = folder.strip_prefix(root)?;
    let mut resolved = canonical_root.to_path_buf();
    for part in relative.components() {
        let Component::Normal(part) = part else {
            anyhow::bail!("{} lies outside the category's folder", folder.display());
        };
        resolved.push(part);
        match tokio::fs::symlink_metadata(&resolved).await {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => anyhow::bail!(
                "{} is a link or a file, not a folder below the category's folder",
                resolved.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match tokio::fs::create_dir(&resolved).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    let target = resolved.join(name);
    let Ok(existing) = tokio::fs::symlink_metadata(&target).await else {
        rd_files::move_file(from, &target).await?;
        return Ok(Placed::Moved);
    };
    match policy {
        CollisionPolicy::Skip | CollisionPolicy::Ask => Ok(Placed::Left),
        CollisionPolicy::Overwrite if existing.is_file() => {
            rd_files::move_file(from, &target).await?;
            Ok(Placed::Replaced)
        }
        CollisionPolicy::Compare if existing.is_file() && same_content(from, &target).await => {
            tokio::fs::remove_file(from).await?;
            Ok(Placed::Identical)
        }
        CollisionPolicy::Rename | CollisionPolicy::Overwrite | CollisionPolicy::Compare => {
            let free = rd_files::collision_free_path(&resolved, &name.to_string_lossy());
            rd_files::move_file(from, &free).await?;
            Ok(Placed::Moved)
        }
    }
}

/// Same size and the same SHA-256; a file that cannot be read is not the same.
async fn same_content(left: &Path, right: &Path) -> bool {
    let sizes = (
        tokio::fs::metadata(left).await.map(|meta| meta.len()),
        tokio::fs::metadata(right).await.map(|meta| meta.len()),
    );
    if !matches!(sizes, (Ok(a), Ok(b)) if a == b) {
        return false;
    }
    let algorithm = rd_core::ChecksumAlgorithm::Sha256;
    match (
        rd_files::compute_checksum(left, algorithm).await,
        rd_files::compute_checksum(right, algorithm).await,
    ) {
        (Ok(a), Ok(b)) => a.value == b.value,
        _ => false,
    }
}

async fn audit_replaced(inner: &Inner, run: &Run<'_>, path: &Path) {
    let mut record =
        rd_db::NewAuditRecord::new(AuditAction::FileOverwritten, AuditOutcome::Success);
    record.target_kind = Some("package".to_owned());
    record.target_id = Some(run.package_id.to_string());
    record.target_name = Some(rd_core::redact_text(&run.package.name));
    record.details.insert(
        "path".to_owned(),
        rd_core::redact_text(&path.to_string_lossy()),
    );
    record
        .details
        .insert("decided_by".to_owned(), "sort".to_owned());
    if let Err(error) = inner.database.append_audit_record(record).await {
        tracing::warn!(package_id = %run.package_id, %error, "recording a replaced file in the audit log failed");
    }
}

/// `sample` as a word of the name or of a folder it sits in: a sample is never the episode,
/// whatever the cleanup settings say, and sorting one would claim the episode's name first.
fn is_sample(name: &str) -> bool {
    name.to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|word| word == "sample")
}

#[cfg(test)]
mod tests {
    use super::is_sample;

    #[test]
    fn samples_are_recognised_by_their_name() {
        assert!(is_sample("Show.S01E01.720p-sample.mkv"));
        assert!(is_sample("Sample/show.s01e01.mkv"));
        assert!(!is_sample("Resample.S01E01.mkv"));
        assert!(!is_sample("Samples.of.Life.S01E01.mkv"));
    }
}
