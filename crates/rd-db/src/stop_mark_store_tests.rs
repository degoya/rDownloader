//! The stop mark's row (RD-1210-02): one at most, replaced, kept across a reopen, cleared only
//! while it still names the target asked about, and gone with its file or package.

use rd_core::{AuthProfileSelection, DownloadId, PackageId};

use crate::{Database, NewDownload, NewPackage, StopMarkTarget};

async fn file(database: &Database, directory: &std::path::Path) -> (PackageId, DownloadId) {
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: format!("stop mark {package_id}"),
            destination: directory.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let download = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "https://example.test/stop-mark.bin".parse().expect("URL"),
            file_name: "stop-mark.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    (package_id, download.id)
}

#[tokio::test]
async fn one_mark_at_most_kept_across_a_reopen() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("stop-mark.sqlite3");
    let database = Database::open(&path).await.expect("database");
    assert_eq!(database.stop_mark().await.expect("read"), None);
    let (package, first) = file(&database, directory.path()).await;

    database
        .set_stop_mark(StopMarkTarget::Download(first))
        .await
        .expect("set");
    let replaced = database
        .set_stop_mark(StopMarkTarget::Package(package))
        .await
        .expect("replace");
    database.close().await.expect("close");

    let database = Database::open(&path).await.expect("reopen");
    let stored = database.stop_mark().await.expect("read").expect("a mark");
    assert_eq!(stored, replaced, "the second mark replaced the first");
    assert_eq!(stored.target, StopMarkTarget::Package(package));
}

#[tokio::test]
async fn a_conditional_clear_keeps_a_mark_set_anew() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("stop-mark.sqlite3"))
        .await
        .expect("database");
    let (_, first) = file(&database, directory.path()).await;
    let (_, second) = file(&database, directory.path()).await;
    database
        .set_stop_mark(StopMarkTarget::Download(second))
        .await
        .expect("set");

    assert!(
        !database
            .clear_stop_mark(Some(StopMarkTarget::Download(first)))
            .await
            .expect("clear another"),
        "a clear for another target removed the mark"
    );
    assert!(database.stop_mark().await.expect("read").is_some());
    assert!(
        database
            .clear_stop_mark(Some(StopMarkTarget::Download(second)))
            .await
            .expect("clear its own")
    );
    assert_eq!(database.stop_mark().await.expect("read"), None);
    assert!(!database.clear_stop_mark(None).await.expect("nothing left"));
}

#[tokio::test]
async fn the_mark_goes_with_its_file_and_with_its_package() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("stop-mark.sqlite3"))
        .await
        .expect("database");
    let (_, marked_file) = file(&database, directory.path()).await;
    database
        .set_stop_mark(StopMarkTarget::Download(marked_file))
        .await
        .expect("set on the file");
    database.delete_download(marked_file).await.expect("delete");
    assert_eq!(database.stop_mark().await.expect("read"), None);

    // The package goes with its last file, and its mark with it.
    let (package, last_file) = file(&database, directory.path()).await;
    database
        .set_stop_mark(StopMarkTarget::Package(package))
        .await
        .expect("set on the package");
    database.delete_download(last_file).await.expect("delete");
    assert!(database.get_package(package).await.expect("read").is_none());
    assert_eq!(database.stop_mark().await.expect("read"), None);
}
