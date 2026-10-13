//! A package's outcome on the bus (RD-1260-04): a package whose files all failed becomes
//! `failed` once, with the count, and every `package.state` names the state before it, so a
//! notification is decided once per transition.

use std::path::Path;

use rd_core::{
    DownloadId, DownloadKind, DownloadPriority, DownloadState, EventKind, Failure, FailureKind,
    PackageId, PackageState,
};
use rd_db::{Database, NewDownload, NewPackage};
use tokio::sync::broadcast::error::TryRecvError;

async fn open(directory: &Path) -> Database {
    let database = rd_testkit::temp_database(directory).await;
    database
        .install_file_vault(directory.join("secrets"))
        .await
        .expect("vault");
    database
}

async fn package_of(
    database: &Database,
    directory: &Path,
    files: usize,
) -> (PackageId, Vec<DownloadId>) {
    let id = PackageId::new();
    database
        .create_package(NewPackage {
            id,
            name: "Fifty".to_owned(),
            destination: directory.join("Fifty").to_string_lossy().into_owned(),
            category_id: None,
            priority: DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let mut downloads = Vec::new();
    for index in 0..files {
        let download = database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id: id,
                source: url::Url::parse(&format!("https://files.example/{index}.bin"))
                    .expect("url"),
                file_name: format!("part-{index}.bin"),
                total_bytes: Some(rd_core::ByteCount::new(1000).expect("bytes")),
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                initial_state: DownloadState::Queued,
                kind: DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                secret_fragment: None,
                mirror_group: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("download");
        downloads.push(download.id);
    }
    (id, downloads)
}

/// The `package.state` payloads that reached the bus since `events` subscribed.
fn package_states(
    events: &mut tokio::sync::broadcast::Receiver<rd_core::EventEnvelope>,
) -> Vec<serde_json::Value> {
    let mut states = Vec::new();
    loop {
        match events.try_recv() {
            Ok(event) if event.kind == EventKind::PackageState => states.push(event.payload),
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(_) => return states,
        }
    }
}

#[tokio::test]
async fn a_package_whose_fifty_files_fail_fails_once_with_the_count() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let (package, downloads) = package_of(&database, directory.path(), 50).await;
    let mut events = database.subscribe();
    for id in downloads {
        database
            .record_failure(
                id,
                Failure::coded(FailureKind::Offline, "http.not_found", "the file is gone"),
                None,
            )
            .await
            .expect("failed");
    }
    let failed: Vec<_> = package_states(&mut events)
        .into_iter()
        .filter(|payload| payload["state"] == "failed")
        .collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert_eq!(failed[0]["previous"], "queued");
    assert_eq!(failed[0]["files"], 50);
    assert_eq!(failed[0]["failed_files"], 50);
    let stored = database
        .get_package(package)
        .await
        .expect("package")
        .expect("exists");
    assert_eq!(stored.state, PackageState::Failed);
}

#[tokio::test]
async fn a_package_written_twice_names_the_state_before_each_time() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let (package, _) = package_of(&database, directory.path(), 1).await;
    let mut events = database.subscribe();
    for _ in 0..2 {
        database
            .set_package_state(package, PackageState::Completed, None, None, None)
            .await
            .expect("completed");
    }
    let states = package_states(&mut events);
    assert_eq!(states.len(), 2, "{states:?}");
    assert_eq!(states[0]["previous"], "queued");
    assert_eq!(states[1]["previous"], "completed");
    assert_eq!(states[1]["files"], 1);
    assert_eq!(states[1]["failed_files"], 0);
}
