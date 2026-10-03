//! Archive passwords in the vault (RD-190-04): what the file holds, what the vault holds, and
//! what a start does with a database written before the move.

use std::path::Path;

use rd_core::{DownloadPackage, DownloadPriority, ImportMode, IngressSource, PackageId};
use sqlx::{Connection, SqliteConnection};

use crate::{
    Database, NewCollectorBatch, NewNzbFile, NewNzbImport, NewNzbSegment, NewPackage,
    NewSubscription, NewSubscriptionItem, PackageChange,
    archive_password::{self, PasswordTable},
};

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

async fn package(database: &Database, directory: &Path, name: &str) -> DownloadPackage {
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
}

fn password_change(password: Option<&str>) -> PackageChange {
    PackageChange {
        password: Some(password.map(str::to_owned)),
        ..PackageChange::default()
    }
}

fn nzb(name: &str, sha: &str, password: Option<&str>) -> NewNzbImport {
    NewNzbImport {
        name: format!("{name}.nzb"),
        sha256: sha.repeat(32),
        category_id: None,
        source: IngressSource::Manual,
        priority: None,
        import_mode: ImportMode::Review,
        source_path: None,
        password: password.map(str::to_owned),
        announce_arrival: false,
        files: vec![NewNzbFile {
            subject: format!("{name}.bin"),
            poster: "poster".to_owned(),
            groups: vec!["alt.binaries.test".to_owned()],
            segments: vec![NewNzbSegment {
                number: 1,
                bytes: 128,
                message_id: format!("{name}-1@example.test"),
            }],
        }],
    }
}

fn batch(password: Option<&str>) -> NewCollectorBatch {
    NewCollectorBatch {
        package_hints: Vec::new(),
        mirror_hints: Vec::new(),
        source: IngressSource::Manual,
        source_label: None,
        package_name: Some("Release".to_owned()),
        password: password.map(str::to_owned),
        passwords: Vec::new(),
        category_id: None,
        priority: None,
        providers: vec![None],
        urls: vec![
            "https://ddownload.com/abc123/Release.rar"
                .parse()
                .expect("url"),
        ],
        file_names: Vec::new(),
        sizes: Vec::new(),
        requests: Vec::new(),
        body_refs: Vec::new(),
        auto_check: false,
        source_attributes: Vec::new(),
    }
}

fn subscription() -> NewSubscription {
    NewSubscription {
        source_categories: Vec::new(),
        name: "Indexer".to_owned(),
        url: "https://indexer.test/api".parse().expect("url"),
        kind: rd_core::SubscriptionKind::Indexer,
        enabled: true,
        mode: rd_core::SubscriptionMode::Review,
        category_id: None,
        priority: DownloadPriority::default(),
        interval_seconds: 3_600,
        filters: rd_core::SubscriptionFilters::default(),
        backlog: rd_core::BacklogPolicy::FromNow,
        category_map: Vec::new(),
        every_release: false,
        view: rd_core::SubscriptionView::List,
        autoplay: false,
        card_ratio: rd_core::SubscriptionCardRatio::TwoOne,
        schedule: None,
        script_arguments: Vec::new(),
        secret_ref: None,
        indexer_search: rd_core::IndexerSearch::default(),
        git_release: rd_core::GitReleaseOptions::default(),
    }
}

fn item(key: &str, password: Option<&str>) -> NewSubscriptionItem {
    NewSubscriptionItem {
        item_key: key.to_owned(),
        title: format!("Release {key}"),
        url: format!("https://indexer.test/get/{key}.nzb")
            .parse()
            .expect("url"),
        published_at: None,
        duration_seconds: None,
        state: rd_core::SubscriptionItemState::Pending,
        reason: None,
        source_category: None,
        media_type: None,
        attributes: std::collections::BTreeMap::new(),
        password: password.map(str::to_owned),
    }
}

/// Every byte of the database file, after the log was folded into it.
async fn raw_file(database: &Database) -> Vec<u8> {
    database.checkpoint_wal().await.expect("checkpoint");
    std::fs::read(database.path()).expect("database file")
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// Writes a value into the old plain column, as every build before RD-190-04 did.
async fn write_plain(database: &Database, table: PasswordTable, id: &str, value: &str) {
    let mut connection =
        SqliteConnection::connect(&format!("sqlite://{}", database.path().display()))
            .await
            .expect("connect");
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(&mut connection)
        .await
        .expect("busy timeout");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {} SET password = ? WHERE id = ?",
        table.name()
    )))
    .bind(value)
    .bind(id)
    .execute(&mut connection)
    .await
    .expect("plain password");
    connection.close().await.expect("close");
}

async fn reference(database: &Database, table: PasswordTable, id: &str) -> Option<String> {
    archive_password::references(&database.readers, table, &[id.to_owned()])
        .await
        .expect("references")
        .remove(id)
}

async fn sweep_rows(database: &Database) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM archive_password_sweep")
        .fetch_one(&database.readers)
        .await
        .expect("sweep rows")
}

/// The acceptance criterion read literally: a password set through every one of the four
/// paths is in no byte of the database file, and still reads back. Before RD-190-04 each of
/// them wrote the value into its row, so every assertion on the file failed.
#[tokio::test]
async fn a_password_set_on_any_of_the_four_paths_is_not_in_the_database_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let queued = package(&database, directory.path(), "Queued").await;
    database
        .update_packages(
            vec![queued.id],
            password_change(Some("rd190-canary-package")),
        )
        .await
        .expect("package password");
    let (_, grabbed, _) = database
        .add_collector_batch(batch(Some("rd190-canary-grabber")))
        .await
        .expect("batch");
    assert_eq!(grabbed[0].password.as_deref(), Some("rd190-canary-grabber"));
    let import = database
        .add_nzb_import(nzb("Usenet", "ab", Some("rd190-canary-nzb")))
        .await
        .expect("import");
    let feed = database
        .create_subscription(subscription())
        .await
        .expect("subscription");
    database
        .record_subscription_items(feed.id, vec![item("hit", Some("rd190-canary-indexer"))])
        .await
        .expect("poll");

    let file = raw_file(&database).await;
    for canary in [
        "rd190-canary-package",
        "rd190-canary-grabber",
        "rd190-canary-nzb",
        "rd190-canary-indexer",
    ] {
        assert!(!contains(&file, canary), "{canary} is in the database file");
    }

    let packages = database
        .list_packages_with_passwords()
        .await
        .expect("packages");
    assert_eq!(
        packages[0].password.as_deref(),
        Some("rd190-canary-package")
    );
    assert_eq!(
        database
            .package_password(queued.id)
            .await
            .expect("password")
            .as_deref(),
        Some("rd190-canary-package")
    );
    // The scheduler's list carries the flag and never the value.
    let plain = database.list_packages().await.expect("packages");
    assert!(plain[0].has_password);
    assert_eq!(plain[0].password, None);
    assert_eq!(
        database
            .collector_package_password(grabbed[0].id)
            .await
            .expect("password")
            .as_deref(),
        Some("rd190-canary-grabber")
    );
    let imports = database.list_nzb_imports().await.expect("imports");
    assert_eq!(imports[0].id, import.id);
    assert_eq!(imports[0].password.as_deref(), Some("rd190-canary-nzb"));
    let items = database
        .subscription_item_page(feed.id, None, 10, 0)
        .await
        .expect("items")
        .items;
    assert_eq!(items[0].password.as_deref(), Some("rd190-canary-indexer"));
}

/// The start of the owner's installation: rows written before the move hold their passwords
/// in plain. After the takeover the plain columns are empty, the file holds none of the values,
/// and every one of them reads back from the vault.
#[tokio::test]
async fn the_takeover_moves_every_plain_password_into_the_vault_and_out_of_the_file() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let queued = package(&database, directory.path(), "Queued").await;
    let empty = package(&database, directory.path(), "Empty").await;
    let (_, grabbed, _) = database
        .add_collector_batch(batch(None))
        .await
        .expect("batch");
    let import = database
        .add_nzb_import(nzb("Usenet", "cd", None))
        .await
        .expect("import");
    let feed = database
        .create_subscription(subscription())
        .await
        .expect("subscription");
    database
        .record_subscription_items(feed.id, vec![item("hit", None)])
        .await
        .expect("poll");
    let hit = database
        .subscription_item_page(feed.id, None, 10, 0)
        .await
        .expect("items")
        .items
        .remove(0);
    let rows = [
        (
            PasswordTable::Packages,
            queued.id.to_string(),
            "rd190-legacy-package",
        ),
        (
            PasswordTable::CollectorPackages,
            grabbed[0].id.to_string(),
            "rd190-legacy-grabber",
        ),
        (
            PasswordTable::NzbImports,
            import.id.to_string(),
            "rd190-legacy-nzb",
        ),
        (
            PasswordTable::SubscriptionItems,
            hit.id.to_string(),
            "rd190-legacy-indexer",
        ),
    ];
    for (table, id, value) in &rows {
        write_plain(&database, *table, id, value).await;
    }
    // An empty value is no password, and the vault refuses to store one.
    write_plain(
        &database,
        PasswordTable::Packages,
        &empty.id.to_string(),
        "",
    )
    .await;
    assert!(contains(&raw_file(&database).await, "rd190-legacy-package"));

    assert_eq!(
        database
            .take_over_archive_passwords()
            .await
            .expect("takeover"),
        5
    );

    let file = raw_file(&database).await;
    for (table, id, value) in &rows {
        assert!(
            !contains(&file, value),
            "{value} is still in the database file"
        );
        assert!(
            reference(&database, *table, id).await.is_some(),
            "{value} has no reference"
        );
        assert!(
            archive_password::plain(&database.readers, *table)
                .await
                .expect("plain")
                .is_empty()
        );
        assert_eq!(
            database
                .archive_password(*table, id.clone())
                .await
                .expect("password")
                .as_deref(),
            Some(*value)
        );
    }
    assert!(
        reference(&database, PasswordTable::Packages, &empty.id.to_string())
            .await
            .is_none()
    );
    assert_eq!(sweep_rows(&database).await, 0);
    // A second start finds nothing left to move.
    assert_eq!(
        database.take_over_archive_passwords().await.expect("again"),
        0
    );
}

/// Without a vault there is nowhere for a password to go: the takeover leaves the plain value
/// where it is, and a new password is refused rather than written into the row.
#[tokio::test]
async fn without_a_vault_nothing_moves_and_nothing_is_written_in_plain() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("rdownloader.sqlite3"))
        .await
        .expect("database");
    let queued = package(&database, directory.path(), "Queued").await;
    write_plain(
        &database,
        PasswordTable::Packages,
        &queued.id.to_string(),
        "rd190-kept",
    )
    .await;
    assert_eq!(
        database
            .take_over_archive_passwords()
            .await
            .expect("takeover"),
        0
    );
    assert_eq!(
        archive_password::plain(&database.readers, PasswordTable::Packages)
            .await
            .expect("plain"),
        vec![(queued.id.to_string(), "rd190-kept".to_owned())]
    );
    let refused = database
        .update_packages(vec![queued.id], password_change(Some("rd190-new")))
        .await
        .expect_err("no vault");
    assert!(
        refused
            .to_string()
            .contains(crate::ARCHIVE_PASSWORD_NO_VAULT),
        "{refused:#}"
    );
    assert!(!contains(&raw_file(&database).await, "rd190-new"));
}

/// Each row owns its entry: a package deleted takes its password out of the vault, a
/// password replaced takes the old one out, and saving the same value writes nothing new.
#[tokio::test]
async fn a_package_takes_its_vault_entry_along_when_it_changes_or_goes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let vault = database.secret_vault().expect("vault").clone();
    let queued = package(&database, directory.path(), "Queued").await;
    let id = queued.id.to_string();
    database
        .update_packages(vec![queued.id], password_change(Some("first")))
        .await
        .expect("first");
    let first = reference(&database, PasswordTable::Packages, &id)
        .await
        .expect("first ref");
    database
        .update_packages(vec![queued.id], password_change(Some("first")))
        .await
        .expect("unchanged");
    assert_eq!(
        reference(&database, PasswordTable::Packages, &id)
            .await
            .as_ref(),
        Some(&first)
    );

    let updated = database
        .update_packages(vec![queued.id], password_change(Some("second")))
        .await
        .expect("second");
    assert_eq!(updated[0].password.as_deref(), Some("second"));
    let second = reference(&database, PasswordTable::Packages, &id)
        .await
        .expect("second ref");
    assert_ne!(first, second);
    vault
        .get(&first)
        .await
        .expect_err("the replaced entry is gone");
    vault.get(&second).await.expect("the new entry is there");

    let cleared = database
        .update_packages(vec![queued.id], password_change(None))
        .await
        .expect("clear");
    assert!(!cleared[0].has_password);
    vault
        .get(&second)
        .await
        .expect_err("the cleared entry is gone");

    database
        .update_packages(vec![queued.id], password_change(Some("third")))
        .await
        .expect("third");
    let third = reference(&database, PasswordTable::Packages, &id)
        .await
        .expect("third ref");
    assert!(
        database
            .delete_empty_package(queued.id)
            .await
            .expect("delete")
    );
    vault
        .get(&third)
        .await
        .expect_err("the deleted package's entry is gone");
    assert_eq!(sweep_rows(&database).await, 0);
}

/// An NZB import hands its password to the package it becomes as a copy of its own: forgetting
/// the import's history must not take the package's password with it.
#[tokio::test]
async fn a_queued_nzb_keeps_its_password_when_the_import_is_forgotten() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let import = database
        .add_nzb_import(nzb("Usenet", "ef", Some("rd190-handed-over")))
        .await
        .expect("import");
    let queued = database
        .enqueue_nzb_import(
            import.id,
            directory.path().join("out"),
            DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue");
    assert_eq!(queued.password.as_deref(), Some("rd190-handed-over"));
    let import_ref = reference(&database, PasswordTable::NzbImports, &import.id.to_string())
        .await
        .expect("import ref");
    let package_ref = reference(&database, PasswordTable::Packages, &queued.id.to_string())
        .await
        .expect("package ref");
    assert_ne!(import_ref, package_ref);

    database
        .forget_nzb_import_history(queued.id)
        .await
        .expect("forget");
    let vault = database.secret_vault().expect("vault");
    vault
        .get(&import_ref)
        .await
        .expect_err("the import's entry went with it");
    assert_eq!(
        database
            .package_password(queued.id)
            .await
            .expect("password")
            .as_deref(),
        Some("rd190-handed-over")
    );
}

/// The copies a start or an update wrote before the move lose their plain passwords, and a
/// copy without any is left as it was.
#[tokio::test]
async fn a_copy_written_before_the_move_loses_its_plain_passwords() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path()).await;
    let queued = package(&database, directory.path(), "Queued").await;
    write_plain(
        &database,
        PasswordTable::Packages,
        &queued.id.to_string(),
        "rd190-in-a-copy",
    )
    .await;
    let copies = directory.path().join(crate::pre_migration::DIRECTORY);
    std::fs::create_dir_all(&copies).expect("copies");
    let copy = copies.join(crate::pre_migration::copy_name(
        "0111",
        "0113",
        chrono::Utc::now(),
    ));
    database.snapshot_into(&copy).await.expect("copy");
    let unrelated = copies.join("notes.sqlite3");
    std::fs::copy(&copy, &unrelated).expect("unrelated file");
    assert!(contains(
        &std::fs::read(&copy).expect("copy"),
        "rd190-in-a-copy"
    ));

    assert_eq!(
        crate::pre_migration::scrub_archive_passwords(&copies).await,
        1
    );
    assert!(!contains(
        &std::fs::read(&copy).expect("copy"),
        "rd190-in-a-copy"
    ));
    // Only the files the copies are named like.
    assert!(contains(
        &std::fs::read(&unrelated).expect("unrelated"),
        "rd190-in-a-copy"
    ));
    assert_eq!(
        crate::pre_migration::scrub_archive_passwords(&copies).await,
        0
    );
}
