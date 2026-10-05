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

/// Owner, 2026-10-02: `K` removes what succeeded. A package whose files all came down but
/// whose unpack failed is a failure and stays until it is removed on purpose.
#[test]
fn a_package_whose_post_processing_failed_stays_on_clear_finished() {
    let package = package(PackageState::Failed, None);
    let downloads = vec![file(package.id, DownloadState::Completed)];
    let packages = vec![package.clone()];

    let finished = plan(&packages, &downloads, PackageClearScope::Completed);
    assert!(
        finished.targets.is_empty(),
        "a failed unpack is not finished"
    );
    assert_eq!(reasons(&finished), vec!["package.postprocess_failed"]);

    let failed = plan(&packages, &downloads, PackageClearScope::Failed);
    assert_eq!(failed.targets, vec![package.id]);
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

/// "Clear the entire list" (RD-180-21) takes what `All` spares — running, waiting, seeding
/// and empty packages — and stops it itself; only post-processing keeps its package.
#[test]
fn clearing_the_entire_list_takes_working_packages_and_spares_post_processing() {
    let running = package(PackageState::Downloading, None);
    let waiting = package(PackageState::Queued, None);
    let seeding = package(PackageState::Completed, None);
    let done = package(PackageState::Completed, None);
    let empty = package(PackageState::Completed, None);
    let unpacking = package(PackageState::Postprocessing, None);
    let downloads = vec![
        file(running.id, DownloadState::Downloading),
        file(running.id, DownloadState::Completed),
        file(waiting.id, DownloadState::Queued),
        file(seeding.id, DownloadState::Seeding),
        file(done.id, DownloadState::Completed),
        file(unpacking.id, DownloadState::Completed),
    ];
    let packages = vec![
        running.clone(),
        waiting.clone(),
        seeding.clone(),
        done.clone(),
        empty.clone(),
        unpacking,
    ];

    let plan = plan(&packages, &downloads, PackageClearScope::Everything);

    assert_eq!(
        plan.targets,
        vec![running.id, waiting.id, seeding.id, done.id, empty.id]
    );
    assert_eq!(reasons(&plan), vec!["package.postprocess_running"]);
}

#[test]
fn a_package_waiting_for_extraction_survives_clearing_the_entire_list() {
    let package = package(PackageState::Completed, None);
    let downloads = vec![file(package.id, DownloadState::Completed)];
    let pending = HashSet::from([package.id]);

    let plan = clear_targets(
        &[package],
        &downloads,
        &pending,
        PackageClearScope::Everything,
    );

    assert!(plan.targets.is_empty());
    assert_eq!(reasons(&plan), vec!["package.postprocess_running"]);
}

/// What the clear stops first: every state a dispatcher or worker could still act on. Idle
/// states are left to the removal, and seeding goes through the torrent engine.
#[test]
fn only_running_and_waiting_members_are_cancelled_before_the_clear() {
    for state in [
        DownloadState::Queued,
        DownloadState::RetryWait,
        DownloadState::Resolving,
        DownloadState::Downloading,
        DownloadState::Verifying,
        DownloadState::Repairing,
        DownloadState::Extracting,
    ] {
        assert!(must_cancel(state), "{state} must be stopped first");
    }
    for state in [
        DownloadState::Paused,
        DownloadState::Blocked,
        DownloadState::Failed,
        DownloadState::Cancelled,
        DownloadState::Completed,
        DownloadState::Skipped,
        DownloadState::Seeding,
    ] {
        assert!(!must_cancel(state), "{state} is not cancelled");
    }
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
