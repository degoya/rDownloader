//! The download window of a package and of a category, stored and read back (RD-1240-30).

use rd_core::{DownloadPriority, DownloadWindow, PackageId, StorageRootId, WeeklyWindow};

use crate::{Database, NewCategory, NewPackage, NewStorageRoot};

fn nightly(ignore_schedule_pause: bool) -> DownloadWindow {
    DownloadWindow {
        windows: vec![WeeklyWindow {
            days: 0b0111_1111,
            start_minute: 22 * 60,
            end_minute: 6 * 60,
        }],
        ignore_schedule_pause,
    }
}

#[tokio::test]
async fn a_package_window_survives_a_reopen_and_can_be_removed() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("window.sqlite3");
    let database = Database::open(&path).await.expect("database");
    let package = NewPackage {
        id: PackageId::new(),
        name: "Tonight".to_owned(),
        destination: directory
            .path()
            .join("Tonight")
            .to_string_lossy()
            .into_owned(),
        category_id: None,
        priority: DownloadPriority::default(),
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let id = package.id;
    let created = database.create_package(package).await.expect("package");
    assert_eq!(created.download_window, None);

    assert!(
        database
            .set_package_download_window(id, Some(nightly(true)))
            .await
            .expect("set")
    );
    database.close().await.expect("close");

    let database = Database::open(&path).await.expect("reopen");
    let read = database
        .get_package(id)
        .await
        .expect("read")
        .expect("there");
    assert_eq!(read.download_window, Some(nightly(true)));

    assert!(
        database
            .set_package_download_window(id, None)
            .await
            .expect("clear")
    );
    let read = database
        .get_package(id)
        .await
        .expect("read")
        .expect("there");
    assert_eq!(read.download_window, None);
    assert!(
        !database
            .set_package_download_window(PackageId::new(), Some(nightly(false)))
            .await
            .expect("missing")
    );
}

#[tokio::test]
async fn a_category_window_is_listed_with_the_category_and_kept_by_an_edit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("category.sqlite3"))
        .await
        .expect("database");
    let root = database
        .create_storage_root(
            StorageRootId::new(),
            NewStorageRoot {
                name: "Downloads".to_owned(),
                path: directory.path().to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root")
        .id;
    let input = NewCategory {
        name: "Night".to_owned(),
        color: "#38BDF8".to_owned(),
        storage_root_id: root,
        relative_path: "night".to_owned(),
        is_default: true,
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
    };
    let category = database
        .create_category(input.clone())
        .await
        .expect("category");
    assert!(
        database
            .set_category_download_window(category.id, Some(nightly(false)))
            .await
            .expect("set")
    );
    // The category editor's own save does not carry the window and must not drop it.
    database
        .update_category(category.id, input)
        .await
        .expect("edit");
    let listed = database.list_categories().await.expect("list");
    assert_eq!(listed[0].download_window, Some(nightly(false)));

    assert!(
        database
            .set_category_download_window(category.id, None)
            .await
            .expect("clear")
    );
    let listed = database.list_categories().await.expect("list");
    assert_eq!(listed[0].download_window, None);
    assert!(
        !database
            .set_category_download_window(rd_core::CategoryId::new(), None)
            .await
            .expect("missing")
    );
}
