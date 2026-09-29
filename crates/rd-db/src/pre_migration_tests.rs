//! The copy before an upgrade (RD-170-07): taken on an existing database with something to
//! apply, never on a fresh one, rotated to the newest three, and put back when a migration fails.

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use sqlx::{
    Connection, SqliteConnection,
    migrate::{Migration, MigrationType, Migrator},
    sqlite::SqliteConnectOptions,
};

use crate::{
    Database, MIGRATOR,
    pre_migration::{self, MIGRATION_FAILED, MigrationFailure},
    restore_copy::copy_schema,
};

/// The real chain up to `last`, and after it whatever `extra` adds.
fn chain(last: i64, extra: Vec<Migration>) -> Migrator {
    let mut migrations: Vec<Migration> = MIGRATOR
        .iter()
        .filter(|migration| migration.version <= last)
        .cloned()
        .collect();
    migrations.extend(extra);
    Migrator {
        migrations: Cow::Owned(migrations),
        ..Migrator::DEFAULT
    }
}

/// A database the way release `up_to` left it, with one setting a later check looks for.
async fn installed(path: &Path, up_to: i64) {
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
    .expect("create");
    chain(up_to, Vec::new())
        .run(&mut connection)
        .await
        .expect("migrate");
    sqlx::query(
        "INSERT INTO settings (key, value_json, updated_at) VALUES ('probe', '\"kept\"', 'then')",
    )
    .execute(&mut connection)
    .await
    .expect("seed");
    connection.close().await.expect("close");
}

fn copies(data: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(data.join(pre_migration::DIRECTORY))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                // The journal files reading a copy may leave are not copies.
                .filter(|name| !name.ends_with("-wal") && !name.ends_with("-shm"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

async fn probe(path: &Path) -> Option<String> {
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true))
            .await
            .expect("open");
    let value = sqlx::query_scalar("SELECT value_json FROM settings WHERE key = 'probe'")
        .fetch_optional(&mut connection)
        .await
        .expect("read");
    connection.close().await.ok();
    value
}

fn latest() -> i64 {
    MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .max()
        .expect("migrations")
}

#[tokio::test]
async fn a_fresh_database_gets_no_copy() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("rdownloader.sqlite3"))
        .await
        .expect("open");
    drop(database);
    assert!(!directory.path().join(pre_migration::DIRECTORY).exists());
}

#[tokio::test]
async fn an_upgrade_is_copied_first_and_only_the_newest_three_copies_stay() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let path = data.join("rdownloader.sqlite3");
    installed(&path, 100).await;
    // Three earlier copies, and a file of somebody else's that rotation must not touch.
    let folder = data.join(pre_migration::DIRECTORY);
    std::fs::create_dir_all(&folder).expect("folder");
    for stamp in [
        "20260101T000000000Z",
        "20260201T000000000Z",
        "20260301T000000000Z",
    ] {
        std::fs::write(
            folder.join(format!("rdownloader-0090-to-0095-{stamp}.sqlite3")),
            b"old",
        )
        .expect("old copy");
    }
    std::fs::write(folder.join("notes.txt"), b"mine").expect("foreign file");

    let database = Database::open(&path).await.expect("upgrade");
    drop(database);

    let names = copies(data);
    let ours = format!("rdownloader-0100-to-{:04}-", latest());
    assert_eq!(names.len(), 4, "{names:?}");
    assert!(names.contains(&"notes.txt".to_owned()), "{names:?}");
    assert!(
        !names.iter().any(|name| name.contains("20260101T")),
        "the oldest copy stays: {names:?}"
    );
    let copy = names
        .iter()
        .find(|name| name.starts_with(&ours))
        .map(|name| folder.join(name))
        .expect("the copy of this upgrade");
    // The copy is the database before the upgrade, with its rows.
    assert_eq!(copy_schema(&copy).await.expect("schema").applied, Some(100));
    assert_eq!(probe(&copy).await.as_deref(), Some("\"kept\""));

    // Nothing pending on the next start: no further copy.
    let database = Database::open(&path).await.expect("reopen");
    drop(database);
    assert_eq!(copies(data), names);
}

#[tokio::test]
async fn a_failed_migration_puts_the_database_back_and_names_the_copy() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let path = data.join("rdownloader.sqlite3");
    installed(&path, 100).await;
    // Two real migrations commit before the third fails: the partial state the copy undoes.
    let failing = Migration::new(
        9_999,
        Cow::Borrowed("fails"),
        MigrationType::Simple,
        sqlx::SqlSafeStr::into_sql_str(
            "CREATE TABLE half_done (id INTEGER); INSERT INTO no_such_table VALUES (1);",
        ),
        false,
    );
    let migrator = chain(102, vec![failing]);

    let error = match Database::open_with(&path, &migrator).await {
        Ok(_) => panic!("the failing migration was applied"),
        Err(error) => error,
    };
    let failure = error
        .downcast_ref::<MigrationFailure>()
        .expect("a migration failure");
    assert_eq!(failure.code, MIGRATION_FAILED);
    assert!(failure.restored, "{failure}");
    let snapshot: PathBuf = failure.snapshot.clone().expect("the copy");
    assert!(snapshot.is_file());
    assert!(
        format!("{error:#}").contains(&snapshot.display().to_string()),
        "the error names the copy: {error:#}"
    );

    // The file is the one the previous build left: 0100, its row, no trace of 0101 to 9999.
    let schema = copy_schema(&path).await.expect("schema");
    assert_eq!(schema.applied, Some(100));
    assert_eq!(probe(&path).await.as_deref(), Some("\"kept\""));
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(&path).read_only(true),
    )
    .await
    .expect("open");
    let half: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('half_done', 'download_sources', 'object_storage_profiles')",
    )
    .fetch_one(&mut connection)
    .await
    .expect("tables");
    connection.close().await.ok();
    assert_eq!(half, 0);
    // The copy the database was put back from stays.
    assert_eq!(copies(data).len(), 1);

    // And the previous build's chain opens it as its own.
    let database = Database::open_with(&path, &chain(100, Vec::new()))
        .await
        .expect("the previous version starts");
    drop(database);
}
