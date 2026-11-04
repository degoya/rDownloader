//! The writer keeps the file consistent when a statement fails part way, and outlives a
//! command that panics.

use std::time::Duration;

use rd_core::{ChunkId, DownloadId, DownloadState, PackageId};
use sqlx::{Connection, SqliteConnection};

use crate::{Database, NewDownload, NewPackage, PersistedChunk, commands::WriterCommand, writer};

async fn download(database: &Database) -> DownloadId {
    let package_id = PackageId::new();
    database
        .create_package(NewPackage {
            id: package_id,
            name: "integrity".to_owned(),
            destination: "downloads".to_owned(),
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
            source: "https://example.test/file".parse().expect("URL"),
            file_name: "file".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Paused,
            kind: rd_core::DownloadKind::Http,
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

/// A second connection to the same file, for what a test does behind the facade's back.
async fn side_connection(database: &Database) -> SqliteConnection {
    SqliteConnection::connect(&format!("sqlite://{}", database.path().display()))
        .await
        .expect("side connection")
}

async fn chunk_count(connection: &mut SqliteConnection, id: DownloadId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM chunks WHERE download_id = ?")
        .bind(id.to_string())
        .fetch_one(connection)
        .await
        .expect("count chunks")
}

/// When the reset of the download row fails, the chunk plan stays: a row with committed bytes
/// and no chunks is what the resume path cannot make sense of.
#[tokio::test]
async fn a_failed_transfer_reset_keeps_the_chunk_plan() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    let id = download(&database).await;
    let chunk = ChunkId::new();
    database
        .prepare_transfer(
            id,
            Some(8_192),
            Some("\"v1\"".to_owned()),
            None,
            vec![PersistedChunk {
                id: chunk,
                start: 0,
                end: Some(8_191),
                committed: 0,
            }],
        )
        .await
        .expect("transfer");
    database
        .checkpoint_chunk(chunk, 1_024)
        .await
        .expect("checkpoint");

    let mut side = side_connection(&database).await;
    sqlx::query(
        "CREATE TRIGGER fail_download_update BEFORE UPDATE ON downloads \
         BEGIN SELECT RAISE(ABORT, 'refused by test'); END",
    )
    .execute(&mut side)
    .await
    .expect("trigger");

    database
        .reset_transfer(id)
        .await
        .expect_err("the row update fails");
    assert_eq!(chunk_count(&mut side, id).await, 1, "the chunks are kept");

    sqlx::query("DROP TRIGGER fail_download_update")
        .execute(&mut side)
        .await
        .expect("drop trigger");
    database.reset_transfer(id).await.expect("reset");
    assert_eq!(
        chunk_count(&mut side, id).await,
        0,
        "a reset that works clears them"
    );
}

/// A command that panics fails on its own; the next command is applied as usual.
#[tokio::test]
async fn the_writer_outlives_a_command_that_panics() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;

    let failed = writer::request(&database.writer, |reply| WriterCommand::PanicForTest {
        reply,
    })
    .await;
    assert!(failed.is_err(), "the caller gets an error, not a hang");
    assert!(database.is_writable());

    let id = download(&database).await;
    assert!(
        database.get_download(id).await.expect("read").is_some(),
        "a write after the panic is persisted"
    );
}

/// Once the writer is gone the facade says so.
#[tokio::test]
async fn a_closed_database_is_not_writable() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = crate::tests::temp_database(directory.path()).await;
    assert!(database.is_writable());
    database.close().await.expect("close");
    // The reply is sent just before the task ends, so the receiver may drop a moment later.
    let mut waited = Duration::ZERO;
    while database.is_writable() && waited < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(10)).await;
        waited += Duration::from_millis(10);
    }
    assert!(!database.is_writable());
}
