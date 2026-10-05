use std::path::PathBuf;

use rd_core::{DownloadKind, PostprocessKind, PostprocessLevel};
use rd_postprocess::group_archive_sets;

use super::{PlanInput, plan};

fn files() -> Vec<PathBuf> {
    [
        "release.par2",
        "release.part1.rar",
        "release.part2.rar",
        "other.zip",
    ]
    .iter()
    .map(|name| PathBuf::from("/pkg").join(name))
    .collect()
}

#[test]
fn plugin_steps_run_after_cleanup_and_before_a_user_script() {
    // By then the package is what it will finally be — unpacked and tidied — so a
    // checksum step sees the files somebody will actually keep. The user script stays
    // the last word, as it always was.
    let files = files();
    let sets = group_archive_sets(&files);
    let steps = ["checksums".to_owned(), "notify".to_owned()];
    let kinds: Vec<PostprocessKind> = plan(&PlanInput {
        level: PostprocessLevel::Delete,
        kind: DownloadKind::Usenet,
        files: &files,
        sets: &sets,
        sfv: &[],
        script: Some("done.sh"),
        cleanup_enabled: true,
        upload: Some("archive:releases"),
        delete_par2: false,
        plugin_steps: &steps,
        malware_scan: false,
        sort: false,
    })
    .into_iter()
    .map(|step| step.kind)
    .collect();
    let cleanup = kinds
        .iter()
        .position(|kind| *kind == PostprocessKind::Cleanup)
        .expect("cleanup");
    let script = kinds
        .iter()
        .position(|kind| *kind == PostprocessKind::Script)
        .expect("script");
    let plugins: Vec<usize> = kinds
        .iter()
        .enumerate()
        .filter(|(_, kind)| **kind == PostprocessKind::PluginStep)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(plugins.len(), 2, "{kinds:?}");
    assert!(plugins[0] > cleanup, "{kinds:?}");
    assert!(plugins[1] < script, "{kinds:?}");
}

#[test]
fn the_malware_scan_runs_after_cleanup_and_before_anything_hands_the_package_on() {
    // RD-190-14: what is scanned is what will be kept, and neither a plugin step, the user
    // script nor the upload sees the package before the scan has had its say.
    let files = files();
    let sets = group_archive_sets(&files);
    let steps = ["checksums".to_owned()];
    let kinds: Vec<PostprocessKind> = plan(&PlanInput {
        level: PostprocessLevel::Delete,
        kind: DownloadKind::Usenet,
        files: &files,
        sets: &sets,
        sfv: &[],
        script: Some("done.sh"),
        cleanup_enabled: true,
        upload: Some("archive:releases"),
        delete_par2: false,
        plugin_steps: &steps,
        malware_scan: true,
        sort: false,
    })
    .into_iter()
    .map(|step| step.kind)
    .collect();
    let at = |wanted: PostprocessKind| {
        kinds
            .iter()
            .position(|kind| *kind == wanted)
            .unwrap_or_else(|| panic!("{wanted:?} missing from {kinds:?}"))
    };
    let scan = at(PostprocessKind::MalwareScan);
    assert!(at(PostprocessKind::Cleanup) < scan, "{kinds:?}");
    assert!(scan < at(PostprocessKind::PluginStep), "{kinds:?}");
    assert!(scan < at(PostprocessKind::Script), "{kinds:?}");
    assert!(scan < at(PostprocessKind::Upload), "{kinds:?}");
}

#[test]
fn the_malware_scan_is_its_own_switch_and_not_a_level() {
    // A package that is not unpacked is still scanned when the switch is on — clamd looks
    // inside the archives — and none is scanned when it is off.
    let files = files();
    let sets = group_archive_sets(&files);
    let kinds = |malware_scan| {
        plan(&PlanInput {
            level: PostprocessLevel::None,
            kind: DownloadKind::Http,
            files: &files,
            sets: &sets,
            sfv: &[],
            script: None,
            cleanup_enabled: true,
            upload: None,
            delete_par2: false,
            plugin_steps: &[],
            malware_scan,
            sort: false,
        })
        .into_iter()
        .map(|step| (step.kind, step.source))
        .collect::<Vec<_>>()
    };
    assert_eq!(
        kinds(true),
        vec![(PostprocessKind::MalwareScan, "clamav".to_owned())]
    );
    assert!(kinds(false).is_empty());
}

#[test]
fn plugin_steps_keep_the_order_they_were_enabled_in() {
    // The list is ordered, and running them in a different order than the one somebody
    // configured would make "first this, then that" impossible to express.
    let files = files();
    let sets = group_archive_sets(&files);
    let steps = ["second".to_owned(), "first".to_owned()];
    let sources: Vec<String> = plan(&PlanInput {
        level: PostprocessLevel::Unpack,
        kind: DownloadKind::Http,
        files: &files,
        sets: &sets,
        sfv: &[],
        script: None,
        cleanup_enabled: false,
        upload: None,
        delete_par2: false,
        plugin_steps: &steps,
        malware_scan: false,
        sort: false,
    })
    .into_iter()
    .filter(|step| step.kind == PostprocessKind::PluginStep)
    .map(|step| step.source)
    .collect();
    assert_eq!(sources, vec!["second".to_owned(), "first".to_owned()]);
}

#[test]
fn the_recovery_set_is_removed_after_unpacking_and_not_after_the_repair() {
    // PAR2 runs before extraction. Deleting the recovery data straight after the repair
    // would leave a package whose unpack then failed with nothing to try again from, so
    // the deletion has to come after the archives are out.
    let files = files();
    let sets = group_archive_sets(&files);
    let kinds: Vec<PostprocessKind> = plan(&PlanInput {
        level: PostprocessLevel::Delete,
        kind: DownloadKind::Usenet,
        files: &files,
        sets: &sets,
        sfv: &[],
        script: None,
        cleanup_enabled: false,
        upload: None,
        delete_par2: true,
        plugin_steps: &[],
        malware_scan: false,
        sort: false,
    })
    .into_iter()
    .map(|step| step.kind)
    .collect();
    let repaired = kinds
        .iter()
        .position(|kind| *kind == PostprocessKind::Par2)
        .expect("par2 step");
    let deleted = kinds
        .iter()
        .position(|kind| *kind == PostprocessKind::DeletePar2)
        .expect("delete step");
    let extracted = kinds
        .iter()
        .position(|kind| matches!(kind, PostprocessKind::ExtractRar))
        .expect("unpack step");
    assert!(repaired < extracted, "{kinds:?}");
    assert!(extracted < deleted, "{kinds:?}");
}

#[test]
fn the_recovery_set_survives_a_level_that_deletes_nothing() {
    // At `+Unpack` the archive volumes stay too; discarding the recovery data while
    // keeping what it protects would be the wrong way round.
    let files = files();
    let sets = group_archive_sets(&files);
    for level in [PostprocessLevel::Repair, PostprocessLevel::Unpack] {
        let kinds: Vec<PostprocessKind> = plan(&PlanInput {
            level,
            kind: DownloadKind::Usenet,
            files: &files,
            sets: &sets,
            sfv: &[],
            script: None,
            cleanup_enabled: false,
            upload: None,
            delete_par2: true,
            plugin_steps: &[],
            malware_scan: false,
            sort: false,
        })
        .into_iter()
        .map(|step| step.kind)
        .collect();
        assert!(
            !kinds.contains(&PostprocessKind::DeletePar2),
            "{level:?}: {kinds:?}"
        );
    }
}

#[test]
fn nothing_is_deleted_unless_it_was_asked_for() {
    let files = files();
    let sets = group_archive_sets(&files);
    let kinds: Vec<PostprocessKind> = plan(&PlanInput {
        level: PostprocessLevel::Delete,
        kind: DownloadKind::Usenet,
        files: &files,
        sets: &sets,
        sfv: &[],
        script: None,
        cleanup_enabled: false,
        upload: None,
        delete_par2: false,
        plugin_steps: &[],
        malware_scan: false,
        sort: false,
    })
    .into_iter()
    .map(|step| step.kind)
    .collect();
    assert!(!kinds.contains(&PostprocessKind::DeletePar2), "{kinds:?}");
}

#[test]
fn levels_gate_the_stages() {
    let files = files();
    let sets = group_archive_sets(&files);
    let kinds = |level, kind, script: Option<&str>| {
        plan(&PlanInput {
            level,
            kind,
            files: &files,
            sets: &sets,
            sfv: &[],
            script,
            cleanup_enabled: true,
            upload: None,
            delete_par2: false,
            plugin_steps: &[],
            malware_scan: false,
            sort: false,
        })
        .into_iter()
        .map(|step| step.kind)
        .collect::<Vec<_>>()
    };
    assert_eq!(
        kinds(PostprocessLevel::None, DownloadKind::Usenet, None),
        Vec::<PostprocessKind>::new()
    );
    // The RAR integrity test joins every level that repairs, for every kind: it is the
    // substitute check for a package nothing else verified, and whether it is needed is
    // only known once PAR2 and SFV have had their say (RD-104-04).
    assert_eq!(
        kinds(PostprocessLevel::Repair, DownloadKind::Usenet, None),
        vec![PostprocessKind::Par2, PostprocessKind::RarTest]
    );
    assert_eq!(
        kinds(PostprocessLevel::Repair, DownloadKind::Http, None),
        vec![PostprocessKind::RarTest]
    );
    assert_eq!(
        kinds(PostprocessLevel::Unpack, DownloadKind::Http, None),
        vec![
            PostprocessKind::RarTest,
            PostprocessKind::ExtractZip,
            PostprocessKind::ExtractRar,
            PostprocessKind::Cleanup
        ]
    );
    assert_eq!(
        kinds(
            PostprocessLevel::Delete,
            DownloadKind::Usenet,
            Some("done.sh")
        ),
        vec![
            PostprocessKind::Par2,
            PostprocessKind::RarTest,
            PostprocessKind::ExtractZip,
            PostprocessKind::DeleteArchives,
            PostprocessKind::ExtractRar,
            PostprocessKind::DeleteArchives,
            PostprocessKind::Cleanup,
            PostprocessKind::Script
        ]
    );
    assert_eq!(
        kinds(PostprocessLevel::None, DownloadKind::Http, Some("done.sh")),
        vec![PostprocessKind::Script]
    );
}

#[test]
fn the_sfv_step_is_planned_between_par2_and_unpack() {
    let files = files();
    let sets = group_archive_sets(&files);
    let index = [PathBuf::from("/pkg/release.sfv")];
    let steps = plan(&PlanInput {
        level: PostprocessLevel::Delete,
        kind: DownloadKind::Usenet,
        files: &files,
        sets: &sets,
        sfv: &index,
        script: None,
        cleanup_enabled: false,
        upload: None,
        delete_par2: false,
        plugin_steps: &[],
        malware_scan: false,
        sort: false,
    });
    let kinds: Vec<PostprocessKind> = steps.iter().map(|step| step.kind).collect();
    assert_eq!(kinds[0], PostprocessKind::Par2);
    assert_eq!(kinds[1], PostprocessKind::Sfv);
    assert_eq!(kinds[2], PostprocessKind::RarTest);
    assert_eq!(kinds[3], PostprocessKind::ExtractZip);
    assert_eq!(steps[1].source, "/pkg/release.sfv");
}

#[test]
fn sfv_verification_runs_for_plain_http_packages_too() {
    let files = files();
    let sets = group_archive_sets(&files);
    let index = [PathBuf::from("/pkg/release.sfv")];
    let kinds = |level| {
        plan(&PlanInput {
            level,
            kind: DownloadKind::Http,
            files: &files,
            sets: &sets,
            sfv: &index,
            script: None,
            cleanup_enabled: false,
            upload: None,
            delete_par2: false,
            plugin_steps: &[],
            malware_scan: false,
            sort: false,
        })
        .into_iter()
        .map(|step| step.kind)
        .collect::<Vec<_>>()
    };
    // PAR2 stays Usenet-only, so `+Repair` on HTTP plans the SFV check and the RAR
    // test that stands in when the SFV index turns out not to answer.
    assert_eq!(
        kinds(PostprocessLevel::Repair),
        vec![PostprocessKind::Sfv, PostprocessKind::RarTest]
    );
    // `None` means no post-processing at all.
    assert_eq!(kinds(PostprocessLevel::None), Vec::<PostprocessKind>::new());
}

#[test]
fn positions_are_strictly_increasing() {
    let files = files();
    let sets = group_archive_sets(&files);
    let steps = plan(&PlanInput {
        level: PostprocessLevel::Delete,
        kind: DownloadKind::Usenet,
        files: &files,
        sets: &sets,
        sfv: &[],
        script: Some("x.sh"),
        cleanup_enabled: true,
        upload: Some("remote:downloads"),
        delete_par2: false,
        plugin_steps: &[],
        malware_scan: false,
        sort: false,
    });
    let positions: Vec<i64> = steps.iter().map(|step| step.position).collect();
    assert_eq!(positions, (1..=positions.len() as i64).collect::<Vec<_>>());
}

#[test]
fn upload_runs_last_and_alone_when_nothing_else_is_planned() {
    let files = files();
    let sets = group_archive_sets(&files);
    let steps = plan(&PlanInput {
        level: PostprocessLevel::Unpack,
        kind: DownloadKind::Http,
        files: &files,
        sets: &sets,
        sfv: &[],
        script: Some("done.sh"),
        cleanup_enabled: true,
        upload: Some("remote:downloads"),
        delete_par2: false,
        plugin_steps: &[],
        malware_scan: false,
        sort: false,
    });
    let last = steps.last().expect("steps");
    assert_eq!(last.kind, PostprocessKind::Upload);
    assert_eq!(last.source, "remote:downloads");

    let only_upload = plan(&PlanInput {
        level: PostprocessLevel::None,
        kind: DownloadKind::Http,
        files: &files,
        sets: &sets,
        sfv: &[],
        script: None,
        cleanup_enabled: false,
        upload: Some("remote:downloads"),
        delete_par2: false,
        plugin_steps: &[],
        malware_scan: false,
        sort: false,
    });
    assert_eq!(
        only_upload.iter().map(|step| step.kind).collect::<Vec<_>>(),
        vec![PostprocessKind::Upload]
    );
}

/// The sort comes after everything else, the upload included, and at every level (RD-1100-08).
#[test]
fn the_sort_runs_last_at_every_level() {
    let files = files();
    let sets = group_archive_sets(&files);
    for level in [
        PostprocessLevel::None,
        PostprocessLevel::Repair,
        PostprocessLevel::Unpack,
        PostprocessLevel::Delete,
    ] {
        let steps = plan(&PlanInput {
            level,
            kind: DownloadKind::Http,
            files: &files,
            sets: &sets,
            sfv: &[],
            script: Some("done.sh"),
            cleanup_enabled: true,
            upload: Some("remote:downloads"),
            delete_par2: false,
            plugin_steps: &[],
            malware_scan: true,
            sort: true,
        });
        let last = steps.last().expect("steps");
        assert_eq!(last.kind, PostprocessKind::Sort, "{level:?}");
        assert_eq!(last.source, crate::sort_job::SOURCE);
        assert_eq!(
            steps
                .iter()
                .filter(|step| step.kind == PostprocessKind::Sort)
                .count(),
            1
        );
    }
}
