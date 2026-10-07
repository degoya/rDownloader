//! Where an NZB import lands: hash dedup, hot folders, rules, failed imports.

use rd_core::{ImportMode, IngressSource};

use super::{dropped_nzb, routing_category, routing_root};
use crate::{
    Database, FailedNzbImport, NewCategory, NewCategoryRule, NewNzbFile, NewNzbImport,
    NewNzbSegment, NewStorageRoot, NzbImportChange,
};

#[tokio::test]
async fn nzb_hash_dedup_preserves_routing_metadata() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb.sqlite"))
        .await
        .expect("database");
    let first = database
        .add_nzb_import(NewNzbImport {
            name: "test.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: Some("/watch/test.nzb".to_owned()),
            password: None,
            announce_arrival: true,
            files: Vec::new(),
        })
        .await
        .expect("first import");
    let duplicate = database
        .add_nzb_import(NewNzbImport {
            name: "copy.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Review,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: Vec::new(),
        })
        .await
        .expect("duplicate import");
    assert_eq!(first.state, rd_core::NzbImportState::Imported);
    assert_eq!(duplicate.import_mode, ImportMode::Enqueue);
    assert!(duplicate.duplicate);
}

/// A rule that only an NZB arriving through the given door can match.
async fn nzb_rule(database: &Database, source: IngressSource, category_id: rd_core::CategoryId) {
    database
        .create_category_rule(NewCategoryRule {
            name: "NZB files".to_owned(),
            priority: 10,
            source: Some(source),
            domain: None,
            protocol: None,
            extension: Some("nzb".to_owned()),
            mime_type: None,
            name_regex: None,
            name_target: rd_core::CategoryRuleNameTarget::File,
            category_id,
            enabled: true,
        })
        .await
        .expect("rule");
}

/// A folder that names a category has decided; neither a matching rule nor the default
/// category may talk it out of that.
#[tokio::test]
async fn a_hotfolder_nzb_keeps_the_category_the_folder_chose() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-explicit.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let chosen = routing_category(&database, root, "Chosen", false).await;
    let by_rule = routing_category(&database, root, "ByRule", false).await;
    routing_category(&database, root, "Fallback", true).await;
    nzb_rule(&database, IngressSource::HotFolder, by_rule.id).await;

    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &"a1".repeat(32),
            Some(chosen.id),
            IngressSource::HotFolder,
            Some("/watch/release.nzb"),
        ))
        .await
        .expect("import");

    assert_eq!(import.category_id, Some(chosen.id));
}

/// The reported defect: a folder without a category left the NZB with none at all, so a rule
/// for `source = hotfolder`, `extension = nzb` could never fire.
#[tokio::test]
async fn an_uncategorised_hotfolder_nzb_takes_the_category_of_a_matching_rule() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-rule.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let by_rule = routing_category(&database, root, "ByRule", false).await;
    routing_category(&database, root, "Fallback", true).await;
    nzb_rule(&database, IngressSource::HotFolder, by_rule.id).await;

    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &"a2".repeat(32),
            None,
            IngressSource::HotFolder,
            Some("/watch/release.nzb"),
        ))
        .await
        .expect("import");
    assert_eq!(import.category_id, Some(by_rule.id));

    // The badge the package shows is the point of the whole exercise.
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("byrule"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    assert_eq!(package.category_id, Some(by_rule.id));
}

fn broken_nzb(name: &str, sha256: &str, error: &str) -> FailedNzbImport {
    FailedNzbImport {
        name: name.to_owned(),
        sha256: sha256.to_owned(),
        source_path: Some(format!("/watch/failed/{name}")),
        error: error.to_owned(),
    }
}

/// RD-108-20: the drop that used to vanish. An NZB nobody threw in by hand could not be
/// parsed, and produced no row at all - so the LinkGrabber showed nothing and the only trace
/// was a log line and a file under `failed/`.
#[tokio::test]
async fn an_unreadable_hotfolder_nzb_is_listed_with_its_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-failed.sqlite"))
        .await
        .expect("database");

    let recorded = database
        .record_nzb_import_failure(broken_nzb(
            "broken.nzb",
            &"f1".repeat(32),
            "NZB could not be parsed",
        ))
        .await
        .expect("failure recorded");

    assert_eq!(recorded.state, rd_core::NzbImportState::Failed);
    assert_eq!(recorded.error.as_deref(), Some("NZB could not be parsed"));

    let listed = database.list_nzb_imports().await.expect("list");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].id, recorded.id);
    assert_eq!(listed[0].name, "broken.nzb");
    assert_eq!(listed[0].state, rd_core::NzbImportState::Failed);
    assert_eq!(listed[0].error.as_deref(), Some("NZB could not be parsed"));
    assert_eq!(
        listed[0].source_path.as_deref(),
        Some("/watch/failed/broken.nzb")
    );
    assert_eq!(listed[0].file_count, 0);

    // Nothing to queue: the row says what went wrong, it is not a candidate.
    let refused = database
        .enqueue_nzb_import(
            recorded.id,
            directory.path().join("downloads"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect_err("enqueue refused");
    assert_eq!(
        crate::store_kind(&refused),
        Some(crate::StoreErrorKind::WrongState),
        "{refused:#}"
    );

    // And it can be cleared, which is what makes room for the file to be imported again.
    database
        .delete_nzb_import(recorded.id)
        .await
        .expect("delete");
    assert!(database.list_nzb_imports().await.expect("list").is_empty());
}

/// The same bytes are the same drop: the reconciliation pass that sees the file again must
/// update the reason, not stack up a second row - `sha256` is unique, so a blind insert would
/// fail outright.
#[tokio::test]
async fn the_same_broken_nzb_arriving_again_updates_the_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-failed-twice.sqlite"))
        .await
        .expect("database");
    let sha256 = "f2".repeat(32);

    let first = database
        .record_nzb_import_failure(broken_nzb("broken.nzb", &sha256, "NZB could not be parsed"))
        .await
        .expect("first failure");
    let second = database
        .record_nzb_import_failure(broken_nzb("broken.nzb", &sha256, "storage root is full"))
        .await
        .expect("second failure");

    assert_eq!(first.id, second.id);
    let listed = database.list_nzb_imports().await.expect("list");
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].error.as_deref(), Some("storage root is full"));
}

/// An import that already became a package worked; a later refusal of the same file - a second
/// drop of a copy, say - must not retract that and mark the package's origin failed.
#[tokio::test]
async fn a_queued_import_is_not_marked_failed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-failed-queued.sqlite"))
        .await
        .expect("database");
    let sha256 = "f3".repeat(32);
    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &sha256,
            None,
            IngressSource::HotFolder,
            Some("/watch/release.nzb"),
        ))
        .await
        .expect("import");
    database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("downloads"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");

    let unchanged = database
        .record_nzb_import_failure(broken_nzb("release.nzb", &sha256, "late refusal"))
        .await
        .expect("recorded");

    assert_eq!(unchanged.state, rd_core::NzbImportState::Enqueued);
    assert_eq!(unchanged.error, None);
}

/// No folder category and no rule that fits: the category marked as default, exactly as a
/// pasted link gets it.
#[tokio::test]
async fn an_uncategorised_hotfolder_nzb_falls_back_to_the_default_category() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-default.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let fallback = routing_category(&database, root, "Fallback", true).await;
    let uploads = routing_category(&database, root, "Uploads", false).await;
    // A rule that cannot match this drop: it is about uploads, not about watched folders.
    nzb_rule(&database, IngressSource::Manual, uploads.id).await;

    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &"a3".repeat(32),
            None,
            IngressSource::HotFolder,
            Some("/watch/release.nzb"),
        ))
        .await
        .expect("import");

    assert_eq!(import.category_id, Some(fallback.id));
}

/// The upload has no path on disk and no folder behind it, and still goes the same way.
#[tokio::test]
async fn an_uploaded_nzb_is_routed_like_a_drop() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-upload.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let by_rule = routing_category(&database, root, "Uploads", false).await;
    routing_category(&database, root, "Fallback", true).await;
    nzb_rule(&database, IngressSource::Manual, by_rule.id).await;

    let import = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &"a4".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("import");

    assert_eq!(import.category_id, Some(by_rule.id));
}

/// The same file dropped into a different folder is routed again instead of keeping what the
/// first drop decided.
#[tokio::test]
async fn an_nzb_dropped_again_adopts_the_category_of_the_second_drop() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-redrop.sqlite"))
        .await
        .expect("database");
    let root = routing_root(&database, directory.path()).await;
    let first_folder = routing_category(&database, root, "Movies", false).await;
    let second_folder = routing_category(&database, root, "Series", false).await;
    let sha256 = "a5".repeat(32);

    let first = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &sha256,
            Some(first_folder.id),
            IngressSource::HotFolder,
            Some("/watch/movies/release.nzb"),
        ))
        .await
        .expect("first import");
    assert_eq!(first.category_id, Some(first_folder.id));

    let again = database
        .add_nzb_import(dropped_nzb(
            "release.nzb",
            &sha256,
            Some(second_folder.id),
            IngressSource::HotFolder,
            Some("/watch/series/release.nzb"),
        ))
        .await
        .expect("second import");

    assert!(again.duplicate);
    assert_eq!(again.id, first.id);
    assert_eq!(again.category_id, Some(second_folder.id));
    assert_eq!(
        database
            .list_nzb_imports()
            .await
            .expect("imports")
            .into_iter()
            .find(|import| import.id == first.id)
            .and_then(|import| import.category_id),
        Some(second_folder.id),
        "the stored row, not just the returned copy, carries the new category"
    );
}

#[tokio::test]
async fn nzb_routing_metadata_can_change_until_enqueue() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("nzb-routing.sqlite"))
        .await
        .expect("database");
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: directory.path().to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    let category = database
        .create_category(NewCategory {
            name: "Changed".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "changed".to_owned(),
            is_default: false,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            unpack_to_subfolder: None,
            unwrap_package_folder: None,
            direct_unpack: None,
            malware_scan: None,
            sfv_verify: None,
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "routing.nzb".to_owned(),
            sha256: "ac".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: Some(rd_core::DownloadPriority::Low),
            import_mode: ImportMode::Review,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "routing.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 128,
                    message_id: "routing-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");

    let updated = database
        .update_nzb_import(
            import.id,
            NzbImportChange {
                category_id: Some(Some(category.id)),
                priority: Some(rd_core::DownloadPriority::High),
            },
        )
        .await
        .expect("update import");
    assert_eq!(updated.category_id, Some(category.id));
    assert_eq!(updated.priority, Some(rd_core::DownloadPriority::High));

    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("changed"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    assert_eq!(package.category_id, Some(category.id));
    assert_eq!(package.priority, rd_core::DownloadPriority::High);
    assert!(
        database
            .update_nzb_import(
                import.id,
                NzbImportChange {
                    category_id: Some(None),
                    priority: None,
                },
            )
            .await
            .is_err(),
        "an enqueued import is edited through its download package instead"
    );
}
