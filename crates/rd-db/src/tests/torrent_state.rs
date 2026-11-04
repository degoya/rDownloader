//! Editing a queue row's torrent state in place: concurrent edits of different fields all
//! survive, and a declined edit writes nothing.

use rd_core::{AuthProfileSelection, DownloadId, PackageId, TorrentFilePriority};

use super::temp_database;
use crate::{Database, NewDownload, NewPackage};

async fn torrent_row(database: &Database, directory: &std::path::Path) -> DownloadId {
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "torrent".to_owned(),
            destination: directory.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: "magnet:?xt=urn:btih:0000000000000000000000000000000000000000"
                .parse()
                .expect("URL"),
            file_name: "torrent".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Torrent,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download")
        .id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_edits_of_different_fields_all_survive() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = temp_database(directory.path()).await;
    let id = torrent_row(&database, directory.path()).await;

    let mut tasks = Vec::new();
    for round in 0..50_u32 {
        let first = database.clone();
        tasks.push(tokio::spawn(async move {
            first
                .update_download_torrent_state(id, move |state| {
                    state.relocation_error = Some(format!("round {round}"));
                    Ok::<_, ()>(())
                })
                .await
        }));
        let second = database.clone();
        tasks.push(tokio::spawn(async move {
            second
                .update_download_torrent_state(id, |state| {
                    state.contract_version += 1;
                    Ok::<_, ()>(())
                })
                .await
        }));
    }
    for task in tasks {
        task.await.expect("task").expect("database").expect("edit");
    }
    let stored = database
        .download_torrent_state(id)
        .await
        .expect("read")
        .expect("state");
    assert!(stored.relocation_error.is_some());
    let start = rd_core::TorrentJobState::default().contract_version;
    assert_eq!(stored.contract_version, start + 50);
}

#[tokio::test]
async fn a_declined_edit_writes_nothing_and_returns_its_reason() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = temp_database(directory.path()).await;
    let id = torrent_row(&database, directory.path()).await;

    let declined = database
        .update_download_torrent_state(id, |state| {
            state.open_tier = Some(TorrentFilePriority::High);
            Err::<(), _>("refused")
        })
        .await
        .expect("database");
    assert_eq!(declined.err(), Some("refused"));
    assert!(
        database
            .download_torrent_state(id)
            .await
            .expect("read")
            .is_none()
    );

    let (stored, value) = database
        .update_download_torrent_state(id, |state| {
            state.open_tier = Some(TorrentFilePriority::High);
            Ok::<_, ()>(7)
        })
        .await
        .expect("database")
        .expect("edit");
    assert_eq!(value, 7);
    assert_eq!(stored.open_tier, Some(TorrentFilePriority::High));
}

#[tokio::test]
async fn an_edit_of_a_missing_row_fails() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = temp_database(directory.path()).await;
    let missing = database
        .update_download_torrent_state(DownloadId::new(), |_| Ok::<_, ()>(()))
        .await;
    assert!(missing.is_err());
}
