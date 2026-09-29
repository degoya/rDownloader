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
    package_handlers::remove_packages,
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
    if package.state == PackageState::Postprocessing || pending_extraction.contains(&package.id) {
        return Some("package.postprocess_running");
    }
    package_progress(downloads, package.id).blocking_code()
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
    // "Everything" includes a package with no members left at all, which `any` would not.
    if scope == PackageClearScope::All {
        return true;
    }
    downloads
        .iter()
        .filter(|file| file.package_id == package_id)
        .any(|file| match scope {
            PackageClearScope::All => true,
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
        if !in_scope(scope, downloads, package.id) {
            continue;
        }
        // Settled once past this: nothing of the package is running, waiting or seeding.
        let reason = blocking_code(package, downloads, pending_extraction).or_else(|| {
            (scope == PackageClearScope::Completed
                && package_progress(downloads, package.id) != PackageProgress::Finished)
                .then_some("package.members_unfinished")
        });
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

#[utoipa::path(post, path = "/api/v1/packages/clear", tag = "downloads", request_body = PackageClearRequest, responses((status = 200, body = PackageClearResponse)))]
pub async fn clear_packages(
    State(state): State<AppState>,
    Json(request): Json<PackageClearRequest>,
) -> Result<Json<PackageClearResponse>, ApiError> {
    let packages = state.database.list_packages().await?;
    let downloads = state.database.list_downloads().await?;
    let pending = state.extraction.pending().await;
    let plan = clear_targets(&packages, &downloads, &pending, request.scope);
    // `force = false` on purpose: every target is settled by construction, and the guard in
    // `remove_packages` is the second lock on the one action that can destroy work.
    let removed = remove_packages(&state, plan.targets, false).await?;
    Ok(Json(PackageClearResponse {
        removed,
        skipped: plan.skipped,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_remove_service::tests::{file, package};

    fn plan(
        packages: &[DownloadPackage],
        downloads: &[DownloadFile],
        scope: PackageClearScope,
    ) -> ClearPlan {
        clear_targets(packages, downloads, &HashSet::new(), scope)
    }

    fn reasons(plan: &ClearPlan) -> Vec<&str> {
        plan.skipped.iter().map(|skip| skip.code.as_str()).collect()
    }

    /// The report: "remove all completed" also removed the finished files of packages that were
    /// still downloading. Against the old per-row store this package lost its completed row,
    /// which is what left the rest of the set with nothing that knew it belonged together.
    #[test]
    fn a_package_with_a_running_member_is_left_whole() {
        let package = package(PackageState::Downloading, None);
        let downloads = vec![
            file(package.id, DownloadState::Completed),
            file(package.id, DownloadState::Downloading),
        ];
        let packages = vec![package.clone()];

        let plan = plan(&packages, &downloads, PackageClearScope::Completed);

        assert!(
            plan.targets.is_empty(),
            "nothing of a running package may be removed"
        );
        assert_eq!(reasons(&plan), vec!["package.members_active"]);
        assert_eq!(plan.skipped[0].package_id, package.id);
        assert_eq!(plan.skipped[0].name, package.name);
    }

    #[test]
    fn a_package_whose_members_all_succeeded_is_removed_whole() {
        let package = package(PackageState::Completed, None);
        let downloads = vec![
            file(package.id, DownloadState::Completed),
            file(package.id, DownloadState::Skipped),
        ];
        let packages = vec![package.clone()];

        let plan = plan(&packages, &downloads, PackageClearScope::Completed);

        assert_eq!(plan.targets, vec![package.id]);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn a_settled_package_with_a_failure_is_not_a_completed_one() {
        let package = package(PackageState::Completed, None);
        let downloads = vec![
            file(package.id, DownloadState::Completed),
            file(package.id, DownloadState::Failed),
        ];
        let packages = vec![package];

        let completed = plan(&packages, &downloads, PackageClearScope::Completed);
        assert!(completed.targets.is_empty());
        assert_eq!(reasons(&completed), vec!["package.members_unfinished"]);

        // The same package under "remove failed" is exactly what that scope is for.
        let failed = plan(&packages, &downloads, PackageClearScope::Failed);
        assert_eq!(failed.targets.len(), 1);
    }

    #[test]
    fn a_package_being_post_processed_is_never_a_target() {
        let unpacking = package(PackageState::Postprocessing, None);
        let downloads = vec![file(unpacking.id, DownloadState::Completed)];
        let by_state = plan(&[unpacking], &downloads, PackageClearScope::All);
        assert_eq!(reasons(&by_state), vec!["package.postprocess_running"]);

        // And in the window before the state is written, the pending set is what knows.
        let queued = package(PackageState::Completed, None);
        let pending = HashSet::from([queued.id]);
        let downloads = vec![file(queued.id, DownloadState::Completed)];
        let waiting = clear_targets(
            &[queued],
            &downloads,
            &pending,
            PackageClearScope::Completed,
        );
        assert!(waiting.targets.is_empty());
        assert_eq!(reasons(&waiting), vec!["package.postprocess_running"]);
    }

    #[test]
    fn a_seeding_torrent_holds_its_package_back_with_its_own_code() {
        let package = package(PackageState::Completed, None);
        let downloads = vec![
            file(package.id, DownloadState::Completed),
            file(package.id, DownloadState::Seeding),
        ];
        let packages = vec![package];

        let plan = plan(&packages, &downloads, PackageClearScope::Completed);

        assert!(plan.targets.is_empty());
        assert_eq!(reasons(&plan), vec!["package.members_seeding"]);
    }

    /// A package the scope does not ask about is not "skipped" — saying so would bury the
    /// packages that really were left alone in a list of ones nobody selected.
    #[test]
    fn a_package_outside_the_scope_is_not_reported_at_all() {
        let package = package(PackageState::Downloading, None);
        let downloads = vec![file(package.id, DownloadState::Downloading)];
        let packages = vec![package];

        let plan = plan(&packages, &downloads, PackageClearScope::Completed);

        assert!(plan.targets.is_empty());
        assert!(plan.skipped.is_empty(), "nothing in it has finished");
    }

    #[test]
    fn clearing_everything_still_spares_what_is_working() {
        let running = package(PackageState::Downloading, None);
        let done = package(PackageState::Completed, None);
        let downloads = vec![
            file(running.id, DownloadState::Downloading),
            file(done.id, DownloadState::Completed),
        ];
        let packages = vec![running, done.clone()];

        let plan = plan(&packages, &downloads, PackageClearScope::All);

        assert_eq!(plan.targets, vec![done.id]);
        assert_eq!(reasons(&plan), vec!["package.members_active"]);
    }

    #[test]
    fn a_blocking_code_always_has_a_refusal_behind_it() {
        for code in [
            "package.members_active",
            "package.members_seeding",
            "package.postprocess_running",
        ] {
            let error = busy_error(code, "Release");
            assert_eq!(error.code(), code, "{code} must survive into the response");
        }
    }
}
