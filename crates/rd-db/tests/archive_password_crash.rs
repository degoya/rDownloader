//! The vault's crash binary. `archive_password.before_reference_adopted` and
//! `archive_password.after_secret_removed` (RD-190-04, recovery matrix): a start that moves the
//! plain archive passwords into the vault stops after the vault holds them and before any row
//! points at them, and a sweep stops after it removed released entries from the vault and before
//! it recorded that. `vault.after_orphan_removed` (DB-03): the sweep at start stops after it
//! removed the first entry nothing names.
#![cfg(feature = "failpoints")]

use std::path::Path;

use rd_core::{DownloadPriority, PackageId, failpoint::FailpointGuard};
use rd_db::{Database, NewPackage, PackageChange};
use sqlx::{Connection, SqliteConnection};

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

async fn connect(directory: &Path) -> SqliteConnection {
    let mut connection = SqliteConnection::connect(&format!(
        "sqlite://{}",
        directory.join("rdownloader.sqlite3").display()
    ))
    .await
    .expect("connect");
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(&mut connection)
        .await
        .expect("busy timeout");
    connection
}

async fn count(directory: &Path, statement: &'static str) -> i64 {
    let mut connection = connect(directory).await;
    let value: i64 = sqlx::query_scalar(statement)
        .fetch_one(&mut connection)
        .await
        .expect("count");
    connection.close().await.expect("close");
    value
}

/// Encrypted entries in the vault, the master key not counted.
fn vault_entries(directory: &Path) -> usize {
    std::fs::read_dir(directory.join("secrets")).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".secret"))
            .count()
    })
}

async fn package(database: &Database, directory: &Path, name: &str) -> PackageId {
    database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: name.to_owned(),
            destination: directory.join(name).to_string_lossy().into_owned(),
            category_id: None,
            priority: DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package")
        .id
}

/// A takeover stopped between the vault and the rows loses no password and leaves no entry
/// nothing points at: the plain values stay, the next start removes what the first one
/// reserved and moves them again, and the vault ends with one entry per password.
#[tokio::test]
async fn a_takeover_stopped_before_the_rows_point_at_the_vault_is_done_again_whole() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    {
        let database = open(data).await;
        for (name, password) in [("First", "rd190-first"), ("Second", "rd190-second")] {
            let id = package(&database, data, name).await;
            let mut connection = connect(data).await;
            sqlx::query("UPDATE packages SET password = ? WHERE id = ?")
                .bind(password)
                .bind(id.to_string())
                .execute(&mut connection)
                .await
                .expect("plain password");
            connection.close().await.expect("close");
        }
        let guard = FailpointGuard::once("archive_password.before_reference_adopted");
        database
            .take_over_archive_passwords()
            .await
            .expect_err("the takeover stops at the crash point");
        assert!(guard.fired());
    }
    assert_eq!(
        count(
            data,
            "SELECT COUNT(*) FROM packages WHERE password IS NOT NULL"
        )
        .await,
        2,
        "a stopped takeover keeps every plain value"
    );
    assert_eq!(
        count(
            data,
            "SELECT COUNT(*) FROM packages WHERE password_ref IS NOT NULL"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            data,
            "SELECT COUNT(*) FROM archive_password_sweep WHERE reserved = 1"
        )
        .await,
        2
    );
    assert_eq!(vault_entries(data), 2);

    let database = open(data).await;
    assert_eq!(
        database
            .take_over_archive_passwords()
            .await
            .expect("takeover"),
        2
    );
    assert_eq!(
        count(
            data,
            "SELECT COUNT(*) FROM packages WHERE password IS NOT NULL"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            data,
            "SELECT COUNT(*) FROM packages WHERE password_ref IS NOT NULL"
        )
        .await,
        2
    );
    assert_eq!(
        count(data, "SELECT COUNT(*) FROM archive_password_sweep").await,
        0
    );
    assert_eq!(
        vault_entries(data),
        2,
        "the first attempt's entries are gone"
    );
    let mut passwords: Vec<String> = database
        .list_packages_with_passwords()
        .await
        .expect("packages")
        .into_iter()
        .filter_map(|package| package.password)
        .collect();
    passwords.sort();
    assert_eq!(passwords, ["rd190-first", "rd190-second"]);
}

/// A sweep stopped after it removed a deleted package's entry and before it recorded that
/// leaves the record, never the entry; the next start finishes it.
#[tokio::test]
async fn a_sweep_stopped_after_the_vault_is_finished_by_the_next_start() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    {
        let database = open(data).await;
        let id = package(&database, data, "Deleted").await;
        database
            .update_packages(
                vec![id],
                PackageChange {
                    password: Some(Some("rd190-deleted".to_owned())),
                    ..PackageChange::default()
                },
            )
            .await
            .expect("password");
        assert_eq!(vault_entries(data), 1);
        let guard = FailpointGuard::once("archive_password.after_secret_removed");
        assert!(database.delete_empty_package(id).await.expect("delete"));
        assert!(guard.fired());
    }
    assert_eq!(vault_entries(data), 0);
    assert_eq!(
        count(data, "SELECT COUNT(*) FROM archive_password_sweep").await,
        1,
        "the removal is not recorded yet"
    );

    let database = open(data).await;
    assert_eq!(
        database.take_over_archive_passwords().await.expect("start"),
        0
    );
    assert_eq!(
        count(data, "SELECT COUNT(*) FROM archive_password_sweep").await,
        0
    );
    assert_eq!(vault_entries(data), 0);
}

/// The sweep at start stopped after its first removal: every entry a row or a settings document
/// names is still there, the next sweep removes the remaining orphans, and a third finds none.
#[tokio::test]
async fn a_vault_sweep_stopped_after_one_removal_is_finished_by_the_next_start() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data = directory.path();
    let named_column;
    let named_setting;
    {
        let database = open(data).await;
        let vault = database.secret_vault().expect("vault").clone();
        named_column = vault.put_string("column".to_owned()).await.expect("put");
        named_setting = vault.put_string("setting".to_owned()).await.expect("put");
        for orphan in ["first", "second", "third"] {
            vault.put_string(orphan.to_owned()).await.expect("orphan");
        }
        database
            .create_account(rd_db::NewAccount {
                provider: "demo".to_owned(),
                label: "Demo".to_owned(),
                username: None,
                credential_mode: None,
                secret_ref: Some(named_column.clone()),
                cookie_ref: None,
                proxy_profile_id: None,
                enabled: true,
            })
            .await
            .expect("account");
        database
            .set_setting(
                "oidc_provider".to_owned(),
                serde_json::json!({ "secret_ref": named_setting }),
            )
            .await
            .expect("setting");
        assert_eq!(vault_entries(data), 5);
        let guard = FailpointGuard::once("vault.after_orphan_removed");
        database
            .sweep_vault()
            .await
            .expect_err("the sweep stops at the crash point");
        assert!(guard.fired());
    }
    assert_eq!(vault_entries(data), 4, "one orphan went before the stop");

    let database = open(data).await;
    assert_eq!(database.sweep_vault().await.expect("sweep"), 2);
    assert_eq!(vault_entries(data), 2);
    let vault = database.secret_vault().expect("vault");
    for named in [&named_column, &named_setting] {
        assert!(vault.get(named).await.is_ok(), "a named entry stays");
    }
    assert_eq!(database.sweep_vault().await.expect("sweep"), 0);
}
