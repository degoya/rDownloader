//! "Clear the download list", as a decision about whole packages.
//!
//! The action used to live entirely in the browser: the store picked every row whose own state
//! was `completed`, cancelled anything active among them and deleted them one at a time, in up
//! to fifteen rounds. Nothing in that chain ever asked what else was in the package, so a
//! package that was still downloading lost the rows of the files it had already finished — and
//! with them everything that knew those files belonged to the set, which is how a half-cleared
//! package reaches post-processing with an archive it can no longer assemble (RD-107-07).
//!
//! So the rule lives here, on the server, in one pure function over whole packages, sharing its
//! member predicate with the timed removal in `auto_remove_service`.

use std::collections::HashSet;

use axum::{Json, extract::State};
use rd_core::{DownloadFile, DownloadPackage, DownloadState, PackageId, PackageState};

use crate::{
    ApiError, AppState,
    auto_remove_service::{PackageProgress, package_progress},
    dto::{PackageClearRequest, PackageClearResponse, PackageClearScope, PackageClearSkip},
    package_handlers::remove_packages_discarding,
};

/// Why this package must not be removed right now, or `None` when it may go.
///
/// Post-processing is asked about first: the package state is written before the pipeline
/// starts and the pending set catches the window in between, and a package whose files are
/// being rewritten is the one case where the member states look harmless and are not.
pub(crate) fn blocking_code(
    package: &DownloadPackage,
    downloads: &[DownloadFile],
    pending_extraction: &HashSet<PackageId>,
) -> Option<&'static str> {
    postprocess_code(package, pending_extraction)
        .or_else(|| package_progress(downloads, package.id).blocking_code())
}

/// The one refusal "clear the entire list" keeps: a running pipeline is no transfer the
/// scheduler could cancel, and removing its rows would leave it rewriting files nobody owns.
fn postprocess_code(
    package: &DownloadPackage,
    pending_extraction: &HashSet<PackageId>,
) -> Option<&'static str> {
    (package.state == PackageState::Postprocessing || pending_extraction.contains(&package.id))
        .then_some("package.postprocess_running")
}

/// The refusal behind a blocking code. Written out per code so the strings stay findable.
pub(crate) fn busy_error(code: &str, name: &str) -> ApiError {
    match code {
        "package.members_seeding" => ApiError::conflict(
            "package.members_seeding",
            "The package is still seeding; stop seeding before removing it",
        ),
        "package.postprocess_running" => ApiError::conflict(
            "package.postprocess_running",
            "The package is being post-processed; wait until that has finished",
        ),
        _ => ApiError::conflict(
            "package.members_active",
            "The package still has running or waiting files; cancel or pause them first",
        ),
    }
    .with_param("name", name)
}

/// What one clear pass decided, worked out before a single row is touched.
pub(crate) struct ClearPlan {
    /// Packages to remove whole.
    pub targets: Vec<PackageId>,
    /// Packages the scope asked for that were left alone, with the reason.
    pub skipped: Vec<PackageClearSkip>,
}

/// Whether the scope is asking about this package at all.
///
/// Deliberately wider than the removal rule: a package that is still downloading but already
/// holds finished files *is* what somebody means by "remove the completed ones". Leaving it out
/// of the selection entirely would let the action pass over it in silence, which is how the
/// old per-file clear managed to gut a running package without saying so.
fn in_scope(scope: PackageClearScope, downloads: &[DownloadFile], package_id: PackageId) -> bool {
    // Both include a package with no members left at all, which `any` would not.
    if matches!(
        scope,
        PackageClearScope::All | PackageClearScope::Everything
    ) {
        return true;
    }
    downloads
        .iter()
        .filter(|file| file.package_id == package_id)
        .any(|file| match scope {
            PackageClearScope::All | PackageClearScope::Everything => true,
            PackageClearScope::Completed => matches!(
                file.state,
                DownloadState::Completed | DownloadState::Seeding
            ),
            PackageClearScope::Failed => matches!(
                file.state,
                DownloadState::Failed | DownloadState::Blocked | DownloadState::Cancelled
            ),
        })
}

/// The whole rule of "clear the download list": whole packages, never single files.
///
/// Pure so the rule can be read and tested without a database. The member predicate is
/// `auto_remove_service::package_progress`, the same one the timed removal uses — the manual
/// path used to have no predicate at all and removed finished rows out of a package that was
/// still downloading, which left its files behind with nothing that knew they belonged
/// together (RD-107-07).
pub(crate) fn clear_targets(
    packages: &[DownloadPackage],
    downloads: &[DownloadFile],
    pending_extraction: &HashSet<PackageId>,
    scope: PackageClearScope,
) -> ClearPlan {
    let mut plan = ClearPlan {
        targets: Vec::new(),
        skipped: Vec::new(),
    };
    for package in packages {
        // A package whose post-processing failed counts as failed, whatever its files did:
        // "remove the finished ones" leaves it in the list for a look at what went wrong
        // (owner, 2026-10-02), and "remove the failed ones" takes it.
        let postprocess_failed = package.state == PackageState::Failed;
        let wanted = match scope {
            PackageClearScope::Failed => {
                postprocess_failed || in_scope(scope, downloads, package.id)
            }
            _ => in_scope(scope, downloads, package.id),
        };
        if !wanted {
            continue;
        }
        // Settled once past this: nothing of the package is running, waiting or seeding —
        // except under `Everything`, which stops all of that itself before it removes.
        let reason = if scope == PackageClearScope::Everything {
            postprocess_code(package, pending_extraction)
        } else {
            blocking_code(package, downloads, pending_extraction)
                .or_else(|| {
                    (scope == PackageClearScope::Completed
                        && package_progress(downloads, package.id) != PackageProgress::Finished)
                        .then_some("package.members_unfinished")
                })
                .or_else(|| {
                    (scope == PackageClearScope::Completed && postprocess_failed)
                        .then_some("package.postprocess_failed")
                })
        };
        match reason {
            Some(code) => plan.skipped.push(PackageClearSkip {
                package_id: package.id,
                name: package.name.clone(),
                code: code.to_owned(),
            }),
            None => plan.targets.push(package.id),
        }
    }
    plan
}

/// Whether a member has to be stopped before "clear the entire list" may remove it: running,
/// or waiting to run. A seeding member is stopped through the torrent engine instead, and
/// everything else is idle already.
fn must_cancel(state: DownloadState) -> bool {
    matches!(
        state,
        DownloadState::Queued
            | DownloadState::RetryWait
            | DownloadState::Resolving
            | DownloadState::Downloading
            | DownloadState::Verifying
            | DownloadState::Repairing
            | DownloadState::Extracting
    )
}

/// Stops every member of the targets that still runs, waits or seeds, before any is removed.
///
/// All first, then the removals: removing one package frees a slot, and the dispatcher would
/// hand it to a queued file of the next target, which then has to be cancelled mid-start. Each
/// step is a transition the scheduler or the engine persists on its own, so a crash anywhere in
/// here leaves cancelled and completed rows — an ordinary queue the next clear finishes. The
/// removal re-checks and waits for each worker to let go (`remove_with_cancel`), so a refusal
/// here is logged rather than returned.
async fn stop_members(state: &AppState, downloads: &[DownloadFile], targets: &[PackageId]) {
    for file in downloads
        .iter()
        .filter(|file| targets.contains(&file.package_id))
    {
        let stopped = if file.state == DownloadState::Seeding {
            state.torrent.stop_seeding(file.id).await.map(|_| ())
        } else if must_cancel(file.state) {
            state.scheduler.cancel(file.id).await
        } else {
            continue;
        };
        if let Err(error) = stopped {
            tracing::debug!(download_id = %file.id, %error, "member was not stopped before the clear");
        }
    }
}

#[utoipa::path(post, path = "/api/v1/packages/clear", tag = "downloads", request_body = PackageClearRequest, responses((status = 200, body = PackageClearResponse), (status = 400), (status = 409)))]
pub async fn clear_packages(
    State(state): State<AppState>,
    audit: crate::audit::AuditContext,
    Json(request): Json<PackageClearRequest>,
) -> Result<Json<PackageClearResponse>, ApiError> {
    let everything = request.scope == PackageClearScope::Everything;
    if everything && !request.confirmed {
        return Err(ApiError::bad_request(
            "package.clear_unconfirmed",
            "Clearing the entire list stops running downloads; confirm it explicitly",
        ));
    }
    let packages = state.database.list_packages().await?;
    let downloads = state.database.list_downloads().await?;
    let pending = state.extraction.pending().await;
    let mut plan = clear_targets(&packages, &downloads, &pending, request.scope);
    if everything {
        stop_members(&state, &downloads, &plan.targets).await;
        // A file that finished while the others were being stopped may have started its
        // package's pipeline; that package stays now, like any other being post-processed.
        let packages = state.database.list_packages().await?;
        let pending = state.extraction.pending().await;
        plan.targets.retain(|id| {
            let Some(package) = packages.iter().find(|package| package.id == *id) else {
                return true;
            };
            let Some(code) = postprocess_code(package, &pending) else {
                return true;
            };
            plan.skipped.push(PackageClearSkip {
                package_id: package.id,
                name: package.name.clone(),
                code: code.to_owned(),
            });
            false
        });
    }
    let targets = plan.targets.clone();
    // Without `Everything`, `force = false` on purpose: every target is settled by
    // construction, and the guard in `remove_packages` is the second lock on the one action
    // that can destroy work. `Everything` has said so explicitly, and confirmed it.
    let removed =
        remove_packages_discarding(&state, plan.targets, everything, request.delete_partial)
            .await?;
    if everything {
        // One record per package, as `delete_packages` writes them.
        for id in targets {
            crate::audit::record(
                &state,
                crate::audit::AuditEvent::success(rd_core::AuditAction::PackageDeleted)
                    .by(&audit)
                    .target("package", id)
                    .detail("forced", true)
                    .detail("clear", "everything")
                    .detail("delete_partial", request.delete_partial),
            )
            .await;
        }
    }
    Ok(Json(PackageClearResponse {
        removed,
        skipped: plan.skipped,
    }))
}

#[cfg(test)]
#[path = "package_clear_tests.rs"]
mod tests;
