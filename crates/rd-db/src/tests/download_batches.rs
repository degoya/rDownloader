//! The list page read in SQL, the removal of many rows in one transaction, and the event a new
//! row announces itself with (RD-1120-17).

use rd_core::{DownloadId, DownloadPriority, DownloadState, EventKind, PackageId};

use super::SELECTION;
use crate::{Database, NewDownload, NewPackage, StoreErrorKind, store_kind};

async fn open(directory: &std::path::Path) -> Database {
    Database::open(directory.join("batches.sqlite"))
        .await
        .expect("database")
}

/// A package of `files` queued rows; answers their ids in queue order.
async fn package(
    database: &Database,
    directory: &std::path::Path,
    name: &str,
    priority: DownloadPriority,
    files: usize,
) -> (PackageId, Vec<DownloadId>) {
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: name.to_owned(),
            destination: directory.join(name).to_string_lossy().into_owned(),
            category_id: None,
            priority,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let mut ids = Vec::with_capacity(files);
    for index in 0..files {
        let row = database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id,
                source: format!("https://example.test/{name}/{index}.bin")
                    .parse()
                    .expect("URL"),
                file_name: format!("{index}.bin"),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: SELECTION,
                initial_state: DownloadState::Queued,
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
        ids.push(row.id);
    }
    (package_id, ids)
}

fn ids(rows: &[rd_core::DownloadFile]) -> Vec<DownloadId> {
    rows.iter().map(|row| row.id).collect()
}

#[tokio::test]
async fn a_page_is_a_slice_of_the_whole_list_and_counts_all_of_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    package(&database, directory.path(), "low", DownloadPriority::Low, 3).await;
    package(
        &database,
        directory.path(),
        "high",
        DownloadPriority::High,
        3,
    )
    .await;
    let whole = ids(&database.list_downloads().await.expect("list"));
    assert_eq!(whole.len(), 6);

    let (rows, total) = database.downloads_page(1, Some(3)).await.expect("page");
    assert_eq!(total, 6);
    assert_eq!(ids(&rows), whole[1..4]);
    let (rows, total) = database.downloads_page(4, None).await.expect("rest");
    assert_eq!(total, 6);
    assert_eq!(ids(&rows), whole[4..], "no limit is the rest of the list");
    let (rows, total) = database.downloads_page(6, Some(3)).await.expect("beyond");
    assert_eq!(total, 6);
    assert!(rows.is_empty(), "a page past the end is empty");
}

#[tokio::test]
async fn many_rows_go_in_one_answer_with_a_refusal_per_row() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let (kept_package, rows) = package(
        &database,
        directory.path(),
        "mixed",
        DownloadPriority::Normal,
        3,
    )
    .await;
    let (gone_package, last) = package(
        &database,
        directory.path(),
        "single",
        DownloadPriority::Normal,
        1,
    )
    .await;
    let working = rows[1];
    database
        .transition_download(working, DownloadState::Resolving)
        .await
        .expect("resolving");
    let mut events = database.subscribe();

    let unknown = DownloadId::new();
    let answers = database
        .delete_downloads(vec![rows[0], unknown, working, rows[2], last[0], rows[0]])
        .await
        .expect("one transaction");
    let kinds: Vec<_> = answers
        .iter()
        .map(|answer| answer.as_ref().err().and_then(store_kind))
        .collect();
    assert_eq!(
        kinds,
        [
            None,
            Some(StoreErrorKind::NotFound),
            Some(StoreErrorKind::WrongState),
            None,
            None,
            Some(StoreErrorKind::NotFound),
        ]
    );
    assert!(answers[0].is_ok() && answers[3].is_ok() && answers[4].is_ok());

    assert_eq!(
        ids(&database.list_downloads().await.expect("list")),
        [working]
    );
    assert!(
        database
            .get_package(kept_package)
            .await
            .expect("read")
            .is_some()
    );
    assert!(
        database
            .get_package(gone_package)
            .await
            .expect("read")
            .is_none(),
        "a package goes with its last file"
    );
    let removed: Vec<_> = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| event.kind == EventKind::DownloadState && event.payload["removed"] == true)
        .collect();
    assert_eq!(removed.len(), 1, "one event for the batch");
    assert_eq!(removed[0].payload["count"], 3);
    assert_eq!(
        removed[0].payload["download_ids"],
        serde_json::json!([rows[0], rows[2], last[0]])
    );
}

/// The rows of one enqueue are announced together: one event however many there are, naming
/// the first hundred and the count, and no transition (RD-1120-17).
#[tokio::test]
async fn the_rows_of_one_enqueue_are_announced_with_one_event() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let mut events = database.subscribe();
    let (package_id, rows) = package(
        &database,
        directory.path(),
        "many",
        DownloadPriority::Normal,
        150,
    )
    .await;
    let created = |events: &mut tokio::sync::broadcast::Receiver<rd_core::EventEnvelope>| {
        std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| event.kind == EventKind::DownloadState)
            .collect::<Vec<_>>()
    };
    assert!(
        created(&mut events).is_empty(),
        "writing a row announces nothing yet"
    );

    database
        .announce_created_downloads(package_id, rows.clone())
        .await
        .expect("announce");
    let announced = created(&mut events);
    assert_eq!(announced.len(), 1, "one event for the whole enqueue");
    let payload = &announced[0].payload;
    assert_eq!(payload["package_id"], package_id.to_string());
    assert_eq!(payload["created"], true);
    assert_eq!(payload["count"], 150);
    assert_eq!(payload["download_ids"].as_array().map(Vec::len), Some(100));
    assert_eq!(payload["download_ids"][0], rows[0].to_string());
    assert!(payload.get("download_id").is_none(), "{payload}");
    assert!(payload.get("state").is_none(), "no transition: {payload}");

    // A single row is named like every other `download.state` event names its row.
    database
        .announce_created_downloads(package_id, vec![rows[0]])
        .await
        .expect("announce one");
    let announced = created(&mut events);
    assert_eq!(announced.len(), 1);
    assert_eq!(announced[0].payload["download_id"], rows[0].to_string());
    assert_eq!(announced[0].payload["count"], 1);
}
