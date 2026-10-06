//! The collision policy, applied where a download meets a name that is already taken
//! (RD-150-01).
//!
//! It is applied twice, and both times the same way. **Before the transfer**, when the name the
//! download will be written under is chosen: a `skip` or an `ask` then costs nothing, and a
//! `compare` whose source stated a checksum can adopt the existing file without fetching a byte.
//! **At the promotion**, because a name that was free when the transfer started can be taken by
//! the time it ends — another download, the user, a script. Until 1.5 the rename out of staging
//! simply replaced whatever had arrived there in between.
//!
//! One policy decides each collision, and it is always the one this module names: an answer
//! somebody gave to this download's prompt, else [`SchedulerHandle::effective_collision_policy`].
//! Two refinements keep a policy from doing damage it was not asked for: a directory in the way
//! is never overwritten or compared (the new file is renamed instead), and an `overwrite` of a
//! file that belongs to a running or seeding transfer turns into a prompt rather than
//! destroying it.

use std::{
    collections::HashMap,
    ops::ControlFlow,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use rd_core::{
    AuditAction, AuditOutcome, CollisionPhase, CollisionPolicy, CollisionPolicySource,
    DownloadFile, DownloadId, EffectiveCollisionPolicy, ExpectedChecksum, PackageId,
};
use rd_db::{NewAuditRecord, NewCollisionPrompt};
use rd_files::collision_free_path;

use crate::{BlockReason, SchedulerHandle};

#[path = "collision_outcomes.rs"]
mod outcomes;

pub(crate) use outcomes::index_finished;
use outcomes::{adopt_after, adopt_before, compare_before, same_content, skip};

/// The stored word of the only algorithm the content index keys on.
pub(crate) const INDEX_ALGORITHM: &str = "sha256";

impl SchedulerHandle {
    /// The policy a collision in this package follows now, and the level that set it.
    pub async fn effective_collision_policy(
        &self,
        package_id: PackageId,
    ) -> Result<EffectiveCollisionPolicy> {
        let levels = self.database.collision_policy_levels(package_id).await?;
        let global = self
            .database
            .service_settings_or_default::<rd_core::StorageSettings>()
            .await?
            .storage_collision_policy;
        Ok(rd_core::effective_collision_policy(
            levels.package,
            levels.category,
            global,
        ))
    }

    /// Whether `path` is, or lies inside, the payload of another download that is running,
    /// being post-processed or seeding. Such a file is never replaced and never linked over.
    pub async fn file_in_use(&self, path: &Path, except: Option<DownloadId>) -> Result<bool> {
        let running = self
            .active
            .lock()
            .await
            .tokens
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let destinations = self
            .database
            .list_packages()
            .await?
            .into_iter()
            .map(|package| (package.id, PathBuf::from(package.destination)))
            .collect::<HashMap<_, _>>();
        Ok(self
            .database
            .list_downloads()
            .await?
            .iter()
            .filter(|other| Some(other.id) != except && !other.file_name.is_empty())
            .filter(|other| other.state.holds_the_file() || running.contains(&other.id))
            .filter_map(|other| {
                destinations
                    .get(&other.package_id)
                    .map(|destination| destination.join(&other.file_name))
            })
            .any(|payload| path.starts_with(&payload)))
    }
}

/// The policy that decides this collision, and whether a person's answer is behind it.
struct Applied {
    policy: CollisionPolicy,
    /// `None` when the policy is an answer to this download's prompt.
    source: Option<CollisionPolicySource>,
}

async fn applicable(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    name: &str,
) -> Result<Applied> {
    // An answer counts only for the name it was given about: after a rename the download
    // collides under a different name, which is a different question.
    if let Some(prompt) = scheduler.database.collision_prompt(file.id).await?
        && prompt.target_name == name
        && let Some(decision) = prompt.decision
    {
        return Ok(Applied {
            policy: decision.policy(),
            source: None,
        });
    }
    let effective = scheduler
        .effective_collision_policy(file.package_id)
        .await?;
    Ok(Applied {
        policy: effective.policy,
        source: Some(effective.source),
    })
}

/// A directory is never replaced by a file and never compared with one.
fn for_existing(policy: CollisionPolicy, existing: &std::fs::Metadata) -> CollisionPolicy {
    if existing.is_dir()
        && matches!(
            policy,
            CollisionPolicy::Overwrite | CollisionPolicy::Compare
        )
    {
        CollisionPolicy::Rename
    } else {
        policy
    }
}

/// Where a fresh transfer writes its file, or `Break` when the policy already ended this
/// attempt (skipped, adopted, waiting for an answer).
///
/// `Overwrite` continues with the taken name: the replacement itself happens at the
/// promotion, where the policy is applied again, the in-use guard is asked again, and the
/// audit record is written.
pub(crate) async fn before_transfer(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    destination: &Path,
    part_path: &Path,
    total_bytes: Option<u64>,
) -> Result<ControlFlow<(), PathBuf>> {
    let current = destination.join(&file.file_name);
    let Ok(existing) = tokio::fs::metadata(&current).await else {
        return Ok(ControlFlow::Continue(current));
    };
    let applied = applicable(scheduler, file, &file.file_name).await?;
    match for_existing(applied.policy, &existing) {
        CollisionPolicy::Rename => Ok(ControlFlow::Continue(
            rename(scheduler, file, destination, &file.file_name).await?,
        )),
        CollisionPolicy::Overwrite => {
            if scheduler.file_in_use(&current, Some(file.id)).await? {
                ask(
                    scheduler,
                    file,
                    &file.file_name,
                    CollisionPhase::BeforeTransfer,
                    Some(existing.len()),
                )
                .await?;
                return Ok(ControlFlow::Break(()));
            }
            Ok(ControlFlow::Continue(current))
        }
        CollisionPolicy::Skip => {
            skip(scheduler, file, part_path).await?;
            Ok(ControlFlow::Break(()))
        }
        CollisionPolicy::Compare => {
            match compare_before(file, &current, existing.len(), total_bytes).await? {
                Some(true) => {
                    adopt_before(scheduler, file, &current, part_path).await?;
                    Ok(ControlFlow::Break(()))
                }
                Some(false) => Ok(ControlFlow::Continue(
                    rename(scheduler, file, destination, &file.file_name).await?,
                )),
                // Nothing to compare with yet: the bytes decide once they are here.
                None => Ok(ControlFlow::Continue(current)),
            }
        }
        CollisionPolicy::Ask => {
            ask(
                scheduler,
                file,
                &file.file_name,
                CollisionPhase::BeforeTransfer,
                Some(existing.len()),
            )
            .await?;
            Ok(ControlFlow::Break(()))
        }
    }
}

/// Where a verified part file is promoted to, and whether that replaces an existing file; or
/// `Break` when the policy ended the attempt here.
pub(crate) async fn after_transfer(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    part_path: &Path,
    final_path: PathBuf,
    computed: Option<&ExpectedChecksum>,
) -> Result<ControlFlow<(), (PathBuf, Option<Overwrite>)>> {
    let Ok(existing) = tokio::fs::metadata(&final_path).await else {
        return Ok(ControlFlow::Continue((final_path, None)));
    };
    let directory = final_path
        .parent()
        .context("final path has no directory")?
        .to_path_buf();
    let name = final_path
        .file_name()
        .and_then(|value| value.to_str())
        .context("final filename is not Unicode")?
        .to_owned();
    let applied = applicable(scheduler, file, &name).await?;
    match for_existing(applied.policy, &existing) {
        CollisionPolicy::Rename => Ok(ControlFlow::Continue((
            rename(scheduler, file, &directory, &name).await?,
            None,
        ))),
        CollisionPolicy::Overwrite => {
            if scheduler.file_in_use(&final_path, Some(file.id)).await? {
                ask(
                    scheduler,
                    file,
                    &name,
                    CollisionPhase::AfterTransfer,
                    Some(existing.len()),
                )
                .await?;
                return Ok(ControlFlow::Break(()));
            }
            let overwrite = Overwrite {
                replaced_bytes: existing.len(),
                source: applied.source,
            };
            Ok(ControlFlow::Continue((final_path, Some(overwrite))))
        }
        CollisionPolicy::Skip => {
            skip(scheduler, file, part_path).await?;
            Ok(ControlFlow::Break(()))
        }
        CollisionPolicy::Compare => {
            if same_content(part_path, &final_path, computed).await? {
                adopt_after(scheduler, file, &final_path, &name, part_path, computed).await?;
                return Ok(ControlFlow::Break(()));
            }
            Ok(ControlFlow::Continue((
                rename(scheduler, file, &directory, &name).await?,
                None,
            )))
        }
        CollisionPolicy::Ask => {
            ask(
                scheduler,
                file,
                &name,
                CollisionPhase::AfterTransfer,
                Some(existing.len()),
            )
            .await?;
            Ok(ControlFlow::Break(()))
        }
    }
}

/// A replacement the promotion is about to perform, for its audit record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Overwrite {
    replaced_bytes: u64,
    /// `None` when a person answered the prompt with `overwrite`.
    source: Option<CollisionPolicySource>,
}

/// Records an overwrite in the audit log, right before the promotion performs it. The actor is
/// the system: the queue does it, on a policy somebody set or an answer somebody gave (that
/// answer has its own record).
pub(crate) async fn audit_overwrite(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    path: &Path,
    overwrite: Overwrite,
) -> Result<()> {
    let mut record = NewAuditRecord::new(AuditAction::FileOverwritten, AuditOutcome::Success);
    record.target_kind = Some("download".to_owned());
    record.target_id = Some(file.id.to_string());
    record.target_name = Some(rd_core::redact_text(&file.file_name));
    record.details.insert(
        "path".to_owned(),
        rd_core::redact_text(&path.to_string_lossy()),
    );
    record.details.insert(
        "replaced_bytes".to_owned(),
        overwrite.replaced_bytes.to_string(),
    );
    record.details.insert(
        "decided_by".to_owned(),
        match overwrite.source {
            None => "prompt",
            Some(CollisionPolicySource::Package) => "package_policy",
            Some(CollisionPolicySource::Category) => "category_policy",
            Some(CollisionPolicySource::Global) => "global_policy",
        }
        .to_owned(),
    );
    scheduler.database.append_audit_record(record).await?;
    Ok(())
}

/// Files the download beside the existing file under the next free `name (n).ext`.
async fn rename(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    directory: &Path,
    name: &str,
) -> Result<PathBuf> {
    let free = collision_free_path(directory, name);
    let free_name = free
        .file_name()
        .and_then(|value| value.to_str())
        .context("collision filename is not Unicode")?
        .to_owned();
    scheduler
        .database
        .set_download_file_name(file.id, free_name)
        .await?;
    Ok(free)
}

/// Opens this download's prompt about `name` and parks it in `Blocked` until somebody answers.
async fn ask(
    scheduler: &SchedulerHandle,
    file: &DownloadFile,
    name: &str,
    phase: CollisionPhase,
    existing_bytes: Option<u64>,
) -> Result<()> {
    scheduler
        .database
        .open_collision_prompt(NewCollisionPrompt {
            download_id: file.id,
            target_name: name.to_owned(),
            phase,
            existing_bytes,
        })
        .await?;
    scheduler
        .database
        .block_download(file.id, BlockReason::CollisionAsk.as_str())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rd_core::CollisionPolicy;

    use super::for_existing;

    #[test]
    fn a_directory_in_the_way_is_never_overwritten_or_compared() {
        let directory = tempfile::tempdir().expect("tempdir");
        let file = directory.path().join("file.bin");
        std::fs::write(&file, b"x").expect("write");
        let as_directory = std::fs::metadata(directory.path()).expect("meta");
        let as_file = std::fs::metadata(&file).expect("meta");
        for policy in CollisionPolicy::ALL {
            let expected = match policy {
                CollisionPolicy::Overwrite | CollisionPolicy::Compare => CollisionPolicy::Rename,
                other => other,
            };
            assert_eq!(for_existing(policy, &as_directory), expected);
            assert_eq!(for_existing(policy, &as_file), policy);
        }
    }
}
