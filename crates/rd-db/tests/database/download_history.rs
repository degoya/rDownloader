//! The download history (RD-1100-04): a package's entry is written with its outcome, outlives
//! the package, survives a restart, follows the retention and the clear, and holds no secret.

use std::path::Path;

use rd_core::{
    DownloadId, DownloadKind, DownloadPriority, DownloadState, Failure, FailureKind,
    HistoryOutcome, PackageId, PackageState,
};
use rd_db::{Database, HistoryQuery, NewDownload, NewPackage, PackageChange};

async fn open(directory: &Path) -> Database {
    let database = Database::open(directory.join("rdownloader.sqlite3"))
        .await
        .expect("database");
    database
        .install_file_vault(directory.join("secrets"))
        .await
        .expect("vault");
    database
}

async fn package_with(
    database: &Database,
    directory: &Path,
    name: &str,
    sources: &[&str],
) -> (PackageId, Vec<DownloadId>) {
    let id = PackageId::new();
    database
        .create_package(NewPackage {
            id,
            name: name.to_owned(),
            destination: directory.join(name).to_string_lossy().into_owned(),
            category_id: None,
            priority: DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let mut downloads = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let download = database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id: id,
                source: url::Url::parse(source).expect("url"),
                file_name: format!("{name}-{index}.bin"),
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

async fn finish(database: &Database, downloads: &[DownloadId]) {
    for id in downloads {
        for state in [
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
        ] {
            database
                .transition_download(*id, state)
                .await
                .expect("transition");
        }
        database
            .complete_download(*id, "done.bin".to_owned(), None)
            .await
            .expect("complete");
    }
}

fn search(text: &str) -> HistoryQuery {
    HistoryQuery {
        search: Some(text.to_owned()),
        ..HistoryQuery::default()
    }
}

#[tokio::test]
async fn a_completed_package_that_was_removed_is_found_by_its_name_after_a_restart() {
    let directory = tempfile::tempdir().expect("tempdir");
    {
        let database = open(directory.path()).await;
        let (package, downloads) = package_with(
            &database,
            directory.path(),
            "Holiday Pictures",
            &["https://files.example/a.bin", "https://files.example/b.bin"],
        )
        .await;
        finish(&database, &downloads).await;
        database
            .set_package_state(package, PackageState::Completed, None, None, None)
            .await
            .expect("completed");
        for id in downloads {
            database.delete_download(id).await.expect("remove");
        }
        assert!(
            database.get_package(package).await.expect("read").is_none(),
            "the package left the queue"
        );
    }
    let database = open(directory.path()).await;
    let page = database
        .list_download_history(&search("holiday"))
        .await
        .expect("history");
    assert_eq!(page.total, 1);
    let entry = &page.entries[0];
    assert_eq!(entry.name, "Holiday Pictures");
    assert_eq!(entry.outcome, HistoryOutcome::Completed);
    assert_eq!(entry.file_count, 2);
    assert_eq!(entry.total_bytes.get(), 2000);
    assert_eq!(entry.error_code, None);
    assert_eq!(
        entry.sources,
        vec![
            "https://files.example/a.bin".to_owned(),
            "https://files.example/b.bin".to_owned()
        ]
    );
    let found = database
        .get_history_entry(entry.id)
        .await
        .expect("read")
        .expect("entry");
    assert_eq!(found.package_id, entry.package_id);
}

#[tokio::test]
async fn a_package_whose_files_all_failed_is_listed_with_the_failure_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let (_, downloads) = package_with(
        &database,
        directory.path(),
        "Broken Release",
        &["https://files.example/gone.bin"],
    )
    .await;
    database
        .record_failure(
            downloads[0],
            Failure::coded(FailureKind::Offline, "http.not_found", "the file is gone"),
            None,
        )
        .await
        .expect("failed");
    let page = database
        .list_download_history(&HistoryQuery {
            outcome: Some(HistoryOutcome::Failed),
            ..HistoryQuery::default()
        })
        .await
        .expect("history");
    assert_eq!(page.total, 1);
    assert_eq!(page.entries[0].name, "Broken Release");
    assert_eq!(
        page.entries[0].error_code.as_deref(),
        Some("http.not_found")
    );
}

#[tokio::test]
async fn a_failure_that_will_be_retried_writes_nothing_and_a_failed_package_keeps_its_code() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let (package, downloads) = package_with(
        &database,
        directory.path(),
        "Retried",
        &["https://files.example/r.bin"],
    )
    .await;
    database
        .record_failure(
            downloads[0],
            Failure::new(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                "reset",
            ),
            Some(chrono::Utc::now() + chrono::Duration::minutes(5)),
        )
        .await
        .expect("retry wait");
    assert_eq!(
        database
            .list_download_history(&HistoryQuery::default())
            .await
            .expect("history")
            .total,
        0
    );
    database
        .set_package_state(package, PackageState::Failed, None, None, None)
        .await
        .expect("post-processing failed");
    let page = database
        .list_download_history(&HistoryQuery::default())
        .await
        .expect("history");
    assert_eq!(page.entries[0].outcome, HistoryOutcome::Failed);
    assert_eq!(
        page.entries[0].error_code.as_deref(),
        Some(rd_db::HISTORY_POSTPROCESS_FAILED_CODE)
    );
    // Retried and finished after all: one entry, now completed.
    database
        .set_package_state(package, PackageState::Completed, None, None, None)
        .await
        .expect("completed");
    let page = database
        .list_download_history(&HistoryQuery::default())
        .await
        .expect("history");
    assert_eq!(page.total, 1);
    assert_eq!(page.entries[0].outcome, HistoryOutcome::Completed);
    assert_eq!(page.entries[0].error_code, None);
}

#[tokio::test]
async fn no_password_and_no_token_reaches_the_history() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let (package, downloads) = package_with(
        &database,
        directory.path(),
        "Canary",
        &[
            "https://user:canary-userinfo@files.example/c.bin?id=1&token=canary-token#canary-key",
            "https://files.example/d.bin?api_key=canary-apikey",
        ],
    )
    .await;
    database
        .update_packages(
            vec![package],
            PackageChange {
                password: Some(Some("canary-archive-password".to_owned())),
                ..PackageChange::default()
            },
        )
        .await
        .expect("password");
    database
        .record_failure(
            downloads[0],
            Failure::coded(
                FailureKind::Offline,
                "http.status",
                "https://files.example/c.bin?token=canary-token answered 404",
            )
            .with_param("url", "https://files.example/c.bin?token=canary-token"),
            None,
        )
        .await
        .expect("failed");
    database
        .record_failure(
            downloads[1],
            Failure::new(FailureKind::Offline, "gone"),
            None,
        )
        .await
        .expect("failed");
    let page = database
        .list_download_history(&HistoryQuery::default())
        .await
        .expect("history");
    assert_eq!(page.total, 1);
    let text = serde_json::to_string(&page.entries).expect("json");
    assert!(!text.contains("canary"), "{text}");
    assert!(text.contains("files.example/c.bin"), "{text}");
}

#[tokio::test]
async fn the_filters_page_and_the_retention_and_the_clear_apply() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    for index in 0..5 {
        let (package, downloads) = package_with(
            &database,
            directory.path(),
            &format!("Series {index}"),
            &["https://files.example/s.bin"],
        )
        .await;
        finish(&database, &downloads).await;
        database
            .set_package_state(package, PackageState::Completed, None, None, None)
            .await
            .expect("completed");
    }
    let page = database
        .list_download_history(&HistoryQuery {
            limit: Some(2),
            offset: 1,
            ..HistoryQuery::default()
        })
        .await
        .expect("page");
    assert_eq!(page.total, 5);
    // Newest first: the second page entry is the fourth package.
    let names: Vec<&str> = page
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, vec!["Series 3", "Series 2"]);
    let none = database
        .list_download_history(&HistoryQuery {
            kind: Some(DownloadKind::Torrent),
            ..HistoryQuery::default()
        })
        .await
        .expect("kind");
    assert_eq!(none.total, 0);
    let later = database
        .list_download_history(&HistoryQuery {
            finished_from: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            ..HistoryQuery::default()
        })
        .await
        .expect("range");
    assert_eq!(later.total, 0);

    // The count cap keeps the newest; the age cap removes everything older than now.
    let removed = database
        .prune_download_history(3, chrono::Utc::now() - chrono::Duration::days(1))
        .await
        .expect("prune");
    assert_eq!(removed, 2);
    let kept = database
        .list_download_history(&HistoryQuery::default())
        .await
        .expect("kept");
    assert_eq!(kept.total, 3);
    assert_eq!(kept.entries[2].name, "Series 2");
    let aged = database
        .prune_download_history(100, chrono::Utc::now() + chrono::Duration::seconds(1))
        .await
        .expect("age");
    assert_eq!(aged, 3);

    let (package, downloads) = package_with(
        &database,
        directory.path(),
        "Last",
        &["https://files.example/l.bin"],
    )
    .await;
    finish(&database, &downloads).await;
    database
        .set_package_state(package, PackageState::Completed, None, None, None)
        .await
        .expect("completed");
    assert_eq!(database.clear_download_history().await.expect("clear"), 1);
    assert_eq!(
        database
            .list_download_history(&HistoryQuery::default())
            .await
            .expect("empty")
            .total,
        0
    );
}

#[tokio::test]
async fn an_entry_a_sabnzbd_client_deleted_stays_in_the_native_history() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let (package, downloads) = package_with(
        &database,
        directory.path(),
        "Imported",
        &["https://files.example/i.bin"],
    )
    .await;
    finish(&database, &downloads).await;
    database
        .set_package_state(package, PackageState::Completed, None, None, None)
        .await
        .expect("completed");
    assert_eq!(
        database
            .hide_history_from_compat(Some(vec![package]))
            .await
            .expect("hide"),
        1
    );
    let compat = database
        .list_download_history(&HistoryQuery {
            compat_visible_only: true,
            ..HistoryQuery::default()
        })
        .await
        .expect("compat");
    assert_eq!(compat.total, 0);
    let native = database
        .list_download_history(&HistoryQuery::default())
        .await
        .expect("native");
    assert_eq!(native.total, 1);
}
