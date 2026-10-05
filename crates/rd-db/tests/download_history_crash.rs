//! The download history's crash binary. `history.before_entry_committed` (RD-1100-04, recovery
//! matrix): the transaction that gives a package its outcome stops after the history entry is
//! written and before it commits.
#![cfg(feature = "failpoints")]

use std::path::Path;

use rd_core::{DownloadPriority, PackageId, PackageState, failpoint::FailpointGuard};
use rd_db::{Database, HistoryQuery, NewPackage};

async fn open(directory: &Path) -> Database {
    Database::open(directory.join("rdownloader.sqlite3"))
        .await
        .expect("database")
}

async fn entries(database: &Database) -> u64 {
    database
        .list_download_history(&HistoryQuery::default())
        .await
        .expect("history")
        .total
}

/// A stop between the entry and the commit leaves neither the outcome nor the entry; the
/// outcome written again leaves exactly one, and it survives the next restart.
#[tokio::test]
async fn an_outcome_stopped_before_its_commit_leaves_no_entry_and_is_recorded_once_again() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let id = PackageId::new();
    {
        let database = open(data).await;
        database
            .create_package(NewPackage {
                id,
                name: "Interrupted".to_owned(),
                destination: data.join("Interrupted").to_string_lossy().into_owned(),
                category_id: None,
                priority: DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let guard = FailpointGuard::once("history.before_entry_committed");
        database
            .set_package_state(id, PackageState::Completed, None, None, None)
            .await
            .expect_err("the outcome stops at the crash point");
        assert!(guard.fired());
    }
    {
        let database = open(data).await;
        let package = database.get_package(id).await.expect("read").expect("kept");
        assert_eq!(
            package.state,
            PackageState::Queued,
            "the package keeps its earlier state"
        );
        assert_eq!(entries(&database).await, 0, "and the history has no entry");
        database
            .set_package_state(id, PackageState::Completed, None, None, None)
            .await
            .expect("completed");
        database
            .set_package_state(id, PackageState::Completed, None, None, None)
            .await
            .expect("completed again");
        assert_eq!(entries(&database).await, 1);
    }
    let database = open(data).await;
    let page = database
        .list_download_history(&HistoryQuery::default())
        .await
        .expect("history");
    assert_eq!(page.total, 1);
    assert_eq!(page.entries[0].package_id, id);
}
