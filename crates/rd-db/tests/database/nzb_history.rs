//! A Usenet file whose package dropped its NZB history (DB-15).
//!
//! `forget_nzb_import_history` deletes the import after a completion when the person keeps no
//! history; the files and segments cascade, and `downloads.nzb_file_id` is set to NULL by its
//! `ON DELETE SET NULL`. The row stays, as the finished file it describes, but there is nothing
//! left to fetch its articles from: a reset is refused instead of queueing a job that can only
//! fail.

use rd_core::{DownloadKind, ImportMode, IngressSource};
use rd_db::{Database, NewNzbFile, NewNzbImport, NewNzbSegment, StoreErrorKind, store_kind};

#[tokio::test]
async fn a_usenet_file_without_its_nzb_keeps_its_row_and_refuses_a_reset() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("history.sqlite"))
        .await
        .expect("database");
    let import = database
        .add_nzb_import(NewNzbImport {
            name: "history.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            source: IngressSource::Manual,
            priority: None,
            import_mode: ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: true,
            files: vec![NewNzbFile {
                subject: "history.bin".to_owned(),
                poster: "poster".to_owned(),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NewNzbSegment {
                    number: 1,
                    bytes: 128,
                    message_id: "history-reset-1@example.test".to_owned(),
                }],
            }],
        })
        .await
        .expect("import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    let files = database
        .downloads_for_package(package.id)
        .await
        .expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].kind, DownloadKind::Usenet);
    assert!(files[0].nzb_file_id.is_some());

    database
        .forget_nzb_import_history(package.id)
        .await
        .expect("forget history");

    let file = database
        .get_download(files[0].id)
        .await
        .expect("read")
        .expect("the row stays");
    assert!(file.nzb_file_id.is_none(), "the cascade cleared the link");
    let error = database
        .reset_download(file.id)
        .await
        .expect_err("nothing left to fetch");
    assert_eq!(store_kind(&error), Some(StoreErrorKind::WrongState));
    let unchanged = database
        .get_download(file.id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(unchanged.state, file.state);
}
