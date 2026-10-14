//! `scheduler.before_package_move` on the automations' path (RD-1290-01): `set_category` wrote
//! the row and stopped before any data moved. The run is not failed for it -- the row keeps the
//! move outstanding -- and the next pass carries the package over into the new category.

use rd_automation::{Action, Trigger};
use rd_core::{DownloadPriority, PackageId, failpoint::FailpointGuard};

use super::execute;
use crate::automation_test_support::context;

/// A category below its own storage root, hidden from the pickers as the owner's was.
async fn hidden_category(
    database: &rd_db::Database,
    directory: &std::path::Path,
) -> rd_core::CategoryId {
    let root = directory.join("library");
    std::fs::create_dir_all(&root).expect("library");
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            rd_db::NewStorageRoot {
                name: "library".to_owned(),
                path: root.to_string_lossy().into_owned(),
                is_default: false,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("storage root");
    database
        .create_category(rd_db::NewCategory {
            name: "NSW - Updates".to_owned(),
            color: "#336699".to_owned(),
            storage_root_id: root.id,
            relative_path: "nsw_updates".to_owned(),
            is_default: false,
            selectable: false,
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
        .expect("category")
        .id
}

#[tokio::test]
async fn a_category_move_stopped_before_the_data_moved_is_finished_by_the_next_pass() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = rd_db::Database::open(directory.path().join("actions.sqlite3"))
        .await
        .expect("database");
    let context = context(&database, directory.path()).await;
    // The start's own pass over outstanding moves, out of the way of the one set up here.
    context.scheduler.storage_recovery_finished().await;
    let category_id = hidden_category(&database, directory.path()).await;
    let old = directory.path().join("downloads").join("Game_Update_NSW");
    std::fs::create_dir_all(&old).expect("package folder");
    std::fs::write(old.join("Game_Update.nsp"), b"unpacked").expect("output");
    let package = database
        .create_package(rd_db::NewPackage {
            id: PackageId::new(),
            name: "Game_Update_NSW".to_owned(),
            destination: old.to_string_lossy().into_owned(),
            category_id: None,
            priority: DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package")
        .id;

    let guard = FailpointGuard::once("scheduler.before_package_move");
    execute(
        &context,
        &Action::SetCategory { category_id },
        Some(package),
        Trigger::PackageCompleted,
        "run:0",
    )
    .await
    .expect("the row is written; the move is left outstanding, not failed");
    assert!(guard.fired(), "the crash point was never reached");
    drop(guard);

    let stored = database
        .get_package(package)
        .await
        .expect("read")
        .expect("package");
    assert_eq!(stored.category_id, Some(category_id));
    assert!(
        old.join("Game_Update.nsp").exists(),
        "data moved before the crash point"
    );
    assert_eq!(
        database
            .package_previous_destination(package)
            .await
            .expect("previous destination")
            .as_deref(),
        Some(old.to_string_lossy().as_ref()),
        "the unfinished move was forgotten"
    );

    // The next pass -- the start's, the next completed file's or the end of post-processing.
    context
        .scheduler
        .relocate_package(package)
        .await
        .expect("the resumed move");

    let new = std::path::PathBuf::from(&stored.destination);
    assert!(
        new.ends_with("nsw_updates/Game_Update_NSW"),
        "{}",
        new.display()
    );
    assert_eq!(
        std::fs::read(new.join("Game_Update.nsp")).expect("moved output"),
        b"unpacked"
    );
    assert!(!old.exists(), "the old package folder stayed behind");
    assert_eq!(
        database
            .package_previous_destination(package)
            .await
            .expect("previous destination"),
        None
    );
}
