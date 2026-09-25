//! Pure planner: which post-processing steps run for a package, in which order.

use std::path::PathBuf;

use rd_core::{DownloadKind, PostprocessKind, PostprocessLevel, PostprocessStage};
use rd_postprocess::{ArchiveSet, is_main_par2};

/// One planned step with its stable pipeline position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlannedStep {
    pub kind: PostprocessKind,
    pub source: String,
    pub position: i64,
    pub stage: PostprocessStage,
}

/// Inputs that decide the pipeline.
pub(crate) struct PlanInput<'a> {
    pub level: PostprocessLevel,
    pub kind: DownloadKind,
    pub files: &'a [PathBuf],
    pub sets: &'a [ArchiveSet],
    /// `.sfv` indexes to verify; empty when the effective setting switches the check off.
    pub sfv: &'a [PathBuf],
    pub script: Option<&'a str>,
    /// Whether the cleanup step is worth scheduling (extensions or samples configured).
    pub cleanup_enabled: bool,
    /// rclone target (`remote:path`) when the effective upload settings are enabled.
    pub upload: Option<&'a str>,
    /// Whether the PAR2 recovery set is removed once unpacking has succeeded.
    pub delete_par2: bool,
    /// Installed plugin steps to run, by plugin id and in the order they should run.
    pub plugin_steps: &'a [String],
}

/// Order: PAR2 (Usenet only) → SFV → unpack → delete archives → delete PAR2 → cleanup →
/// plugin steps → script → upload.
///
/// Plugin steps come after cleanup and before scripts: by then the package is what it will
/// finally be — unpacked and tidied — and a user script stays the last word, as it was.
pub(crate) fn plan(input: &PlanInput<'_>) -> Vec<PlannedStep> {
    let mut steps = Vec::new();
    let mut position = 0_i64;
    let mut push = |kind, source: String, stage| {
        position += 1;
        steps.push(PlannedStep {
            kind,
            source,
            position,
            stage,
        });
    };
    if input.level.repairs() && input.kind == DownloadKind::Usenet {
        for index in input.files.iter().filter(|path| is_main_par2(path)) {
            push(
                PostprocessKind::Par2,
                index.to_string_lossy().into_owned(),
                PostprocessStage::Repairing,
            );
        }
    }
    // Unlike PAR2 this runs for every kind, and before unpacking: at `+Delete` the volumes
    // an index lists are gone once extraction succeeded. `None` means no post-processing at
    // all, so verification starts at `+Repair` like the other integrity step.
    if input.level.repairs() {
        for index in input.sfv {
            push(
                PostprocessKind::Sfv,
                index.to_string_lossy().into_owned(),
                PostprocessStage::Verifying,
            );
        }
        // The substitute check, planned unconditionally so it is visible either way: it runs
        // when neither PAR2 nor an `.sfv` index answered, and is recorded as skipped — with
        // the reason — when one of them did (RD-104-04).
        for set in input
            .sets
            .iter()
            .filter(|set| set.kind == rd_files::ArchiveKind::Rar)
        {
            push(
                PostprocessKind::RarTest,
                set.first().to_string_lossy().into_owned(),
                PostprocessStage::Verifying,
            );
        }
    }
    if input.level.unpacks() {
        for set in input.sets {
            let source = set.first().to_string_lossy().into_owned();
            push(
                archive_kind(set),
                source.clone(),
                PostprocessStage::Extracting,
            );
            if input.level.deletes() {
                push(
                    PostprocessKind::DeleteArchives,
                    source,
                    PostprocessStage::DeletingArchives,
                );
            }
        }
        // After unpacking, never straight after the repair. PAR2 runs first, so deleting the
        // recovery set at repair time would leave a package whose extraction then failed with
        // no way to try again. Removing it here means it survives exactly as long as it could
        // still be needed.
        if input.delete_par2 && input.level.deletes() && input.kind == DownloadKind::Usenet {
            for index in input.files.iter().filter(|path| is_main_par2(path)) {
                push(
                    PostprocessKind::DeletePar2,
                    index.to_string_lossy().into_owned(),
                    PostprocessStage::DeletingPar2,
                );
            }
        }
        if input.cleanup_enabled {
            push(
                PostprocessKind::Cleanup,
                "cleanup".to_owned(),
                PostprocessStage::Cleaning,
            );
        }
    }
    for plugin_id in input.plugin_steps {
        push(
            PostprocessKind::PluginStep,
            plugin_id.clone(),
            PostprocessStage::PluginStep,
        );
    }
    if let Some(script) = input.script {
        push(
            PostprocessKind::Script,
            script.to_owned(),
            PostprocessStage::Script,
        );
    }
    if let Some(remote) = input.upload {
        push(
            PostprocessKind::Upload,
            remote.to_owned(),
            PostprocessStage::Uploading,
        );
    }
    steps
}

pub(crate) const fn archive_kind(set: &ArchiveSet) -> PostprocessKind {
    match set.kind {
        rd_files::ArchiveKind::Zip => PostprocessKind::ExtractZip,
        rd_files::ArchiveKind::SevenZip => PostprocessKind::ExtractSevenZip,
        rd_files::ArchiveKind::Rar => PostprocessKind::ExtractRar,
    }
}

#[cfg(test)]
mod tests {
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
        });
        assert_eq!(
            only_upload.iter().map(|step| step.kind).collect::<Vec<_>>(),
            vec![PostprocessKind::Upload]
        );
    }
}
