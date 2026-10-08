//! The database copy a restore opens (RD-160-03): migrated when older, refused when newer,
//! rewritten all or nothing, and every listed column present in the current schema.

use std::borrow::Cow;

use sqlx::{migrate::Migrator, sqlite::SqliteSynchronous};

use super::*;

/// A copy the way a backup writes one: a whole database, closed.
///
/// Built with the journal in memory and no `fsync` (RD-1120-08): a journal file synced per
/// migration made this 32 s on NTFS. The code under test opens the copy with its own options.
async fn copy_with(directory: &Path, up_to: Option<i64>) -> std::path::PathBuf {
    let path = directory.join("database.sqlite3");
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Memory)
            .synchronous(SqliteSynchronous::Off),
    )
    .await
    .expect("create");
    let full = sqlx::migrate!();
    let migrator = Migrator {
        migrations: Cow::Owned(
            full.iter()
                .filter(|migration| up_to.is_none_or(|last| migration.version <= last))
                .cloned()
                .collect(),
        ),
        ..sqlx::migrate!()
    };
    migrator.run(&mut connection).await.expect("migrate");
    connection.close().await.expect("close");
    path
}

#[tokio::test]
async fn an_older_copy_is_migrated_and_a_current_one_is_left_as_it_is() {
    let directory = tempfile::tempdir().expect("tempdir");
    let copy = copy_with(directory.path(), Some(100)).await;
    let before = copy_schema(&copy).await.expect("schema");
    assert_eq!(before.applied, Some(100));
    assert!(before.pending > 0);
    assert!(!before.is_newer());

    let migrated = migrate_copy(&copy).await.expect("migrate");
    assert_eq!(migrated, before);
    let after = copy_schema(&copy).await.expect("schema");
    assert_eq!(after.applied, Some(after.known));
    assert_eq!(after.pending, 0);
    // Still one file: no journal beside it.
    assert!(!directory.path().join("database.sqlite3-wal").exists());
}

#[tokio::test]
async fn a_copy_from_a_newer_build_is_refused_untouched() {
    let directory = tempfile::tempdir().expect("tempdir");
    let copy = copy_with(directory.path(), None).await;
    let mut connection = open_writable(&copy).await.expect("open");
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, \
         execution_time) VALUES (99999, 'from the future', '2030-01-01', 1, x'00', 0)",
    )
    .execute(&mut connection)
    .await
    .expect("insert");
    connection.close().await.expect("close");
    let schema = copy_schema(&copy).await.expect("schema");
    assert!(schema.is_newer());
    assert_eq!(schema.unknown, vec![99_999]);
    let refused = migrate_copy(&copy).await.expect_err("refused");
    assert!(refused.to_string().contains("newer"), "{refused:#}");
}

#[tokio::test]
async fn cells_are_read_and_rewritten_all_or_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let copy = copy_with(directory.path(), None).await;
    let mut connection = open_writable(&copy).await.expect("open");
    sqlx::query(
        "INSERT INTO storage_roots (id, name, path, is_default, created_at, updated_at) \
         VALUES ('r1', 'Main', 'D:\\Downloads', 1, '2026-01-01', '2026-01-01')",
    )
    .execute(&mut connection)
    .await
    .expect("root");
    connection.close().await.expect("close");

    let cells = read_cells(&copy, &[STORAGE_ROOT_PATH]).await.expect("read");
    assert_eq!(
        cells,
        vec![vec![CopyCell {
            key: "r1".to_owned(),
            value: "D:\\Downloads".to_owned()
        }]]
    );
    apply_updates(
        &copy,
        &[CopyUpdate {
            column: STORAGE_ROOT_PATH,
            key: "r1".to_owned(),
            value: Some("/srv/downloads".to_owned()),
        }],
    )
    .await
    .expect("write");
    // A key that names no row fails the whole batch; the first update does not land.
    let failed = apply_updates(
        &copy,
        &[
            CopyUpdate {
                column: STORAGE_ROOT_PATH,
                key: "r1".to_owned(),
                value: Some("/elsewhere".to_owned()),
            },
            CopyUpdate {
                column: STORAGE_ROOT_PATH,
                key: "missing".to_owned(),
                value: Some("/nowhere".to_owned()),
            },
        ],
    )
    .await;
    assert!(failed.is_err());
    let cells = read_cells(&copy, &[STORAGE_ROOT_PATH]).await.expect("read");
    assert_eq!(cells[0][0].value, "/srv/downloads");
}

#[tokio::test]
async fn every_listed_column_exists_in_the_current_schema() {
    let directory = tempfile::tempdir().expect("tempdir");
    let copy = copy_with(directory.path(), None).await;
    let mut columns = vec![STORAGE_ROOT_PATH, DOWNLOAD_SOURCE, BACKUP_KEY_REF];
    columns.extend_from_slice(PATH_COLUMNS);
    columns.extend_from_slice(BUNDLED_SECRET_COLUMNS);
    columns.extend_from_slice(UNBUNDLED_SECRET_COLUMNS);
    read_cells(&copy, &columns)
        .await
        .expect("every column reads");
    copy_counts(&copy).await.expect("counts");
    assert_eq!(backup_key_of(&copy).await.expect("key"), None);
    assert!(
        dangling_references(&copy)
            .await
            .expect("references")
            .is_empty()
    );
    assert!(
        unfinished_destinations(&copy)
            .await
            .expect("folders")
            .is_empty()
    );
}

#[tokio::test]
async fn a_migrated_older_copy_holds_exactly_the_schema_of_this_build() {
    let directory = tempfile::tempdir().expect("tempdir");
    let copy = copy_with(directory.path(), Some(100)).await;
    migrate_copy(&copy).await.expect("migrate");
    assert_eq!(
        foreign_schema_objects(&copy).await.expect("compare"),
        Vec::<String>::new()
    );
}

/// RD-1190-19: a trigger or a view of a crafted archive would run in the live database after
/// the switch; the copy is refused instead.
#[tokio::test]
async fn a_trigger_and_a_changed_index_of_a_crafted_copy_are_named() {
    let directory = tempfile::tempdir().expect("tempdir");
    let copy = copy_with(directory.path(), None).await;
    let mut connection = open_writable(&copy).await.expect("open");
    let index: String = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL \
         AND tbl_name = 'downloads' ORDER BY name LIMIT 1",
    )
    .fetch_one(&mut connection)
    .await
    .expect("an index of the queue");
    for statement in [
        "CREATE TRIGGER planted AFTER INSERT ON downloads BEGIN DELETE FROM downloads; END"
            .to_owned(),
        format!("DROP INDEX {index}"),
        format!("CREATE INDEX {index} ON downloads (id)"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut connection)
            .await
            .expect("tamper");
    }
    connection.close().await.expect("close");
    let foreign = foreign_schema_objects(&copy).await.expect("compare");
    assert!(
        foreign.contains(&"trigger planted".to_owned()),
        "{foreign:?}"
    );
    assert!(foreign.contains(&format!("index {index}")), "{foreign:?}");
}
