//! Single migrations, each measured on a row written by the schema right before it.
//!
//! These start from an empty database at that schema rather than from the release fixture:
//! each asserts one table, and the fixture's own rows in it would only blur the count.

use sqlx::{Connection, SqliteConnection};

use super::schema_at;

/// A subscription stored before RD-120-37 keeps the list view, and no autoplay, on upgrade.
///
/// The whole promise of migration `0090` is that nobody who changes nothing sees anything new;
/// this is that promise measured on a row written by the schema one migration earlier.
#[tokio::test]
async fn a_subscription_from_before_the_view_choice_keeps_the_list() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 89)
        .await
        .expect("schema at 0089");
    {
        let url = format!("sqlite://{}", path.display());
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        sqlx::query(
            "INSERT INTO subscriptions (id, name, url, kind, enabled, mode, priority,
                                        interval_seconds, created_at, updated_at)
             VALUES ('019d0000-0000-7000-8000-0000000000e1', 'Old search',
                     'https://indexer.example.test/api', 'indexer', 1, 'review', 0, 3600,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&mut connection)
        .await
        .expect("insert a subscription on the 0089 schema");
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    let subscriptions = database.list_subscriptions().await.expect("subscriptions");
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0].view, rd_core::SubscriptionView::List);
    assert!(!subscriptions[0].autoplay);
}

/// A subscription stored before RD-120-42 gets the `2:1` card ratio on upgrade.
///
/// `2:1` is the ratio closest to the fixed height every card had before the choice existed, so
/// migration `0092` changes nothing anybody sees; this measures that on a row written by the
/// schema from before it, with a view and autoplay already chosen that must survive untouched.
#[tokio::test]
async fn a_subscription_from_before_the_card_ratio_gets_two_to_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 90)
        .await
        .expect("schema at 0090");
    {
        let url = format!("sqlite://{}", path.display());
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        sqlx::query(
            "INSERT INTO subscriptions (id, name, url, kind, enabled, mode, priority,
                                        interval_seconds, view, autoplay, created_at, updated_at)
             VALUES ('019d0000-0000-7000-8000-0000000000e2', 'Old music',
                     'https://indexer.example.test/api', 'indexer', 1, 'review', 0, 3600,
                     'cards', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&mut connection)
        .await
        .expect("insert a subscription on the 0090 schema");
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    let subscriptions = database.list_subscriptions().await.expect("subscriptions");
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(
        subscriptions[0].card_ratio,
        rd_core::SubscriptionCardRatio::TwoOne
    );
    assert_eq!(subscriptions[0].view, rd_core::SubscriptionView::Cards);
    assert!(subscriptions[0].autoplay);
}

/// A script subscription stored before RD-150-08 runs its script with no arguments, as before.
///
/// Seeded on the `0098` schema, the last one on this branch below `0103`; `0099`–`0102` belong
/// to parallel branches and none of them touches the subscriptions table.
#[tokio::test]
async fn a_script_subscription_from_before_its_arguments_gets_an_empty_list() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 98)
        .await
        .expect("schema at 0098");
    {
        let url = format!("sqlite://{}", path.display());
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        sqlx::query(
            "INSERT INTO subscriptions (id, name, url, kind, enabled, mode, priority,
                                        interval_seconds, schedule, created_at, updated_at)
             VALUES ('019d0000-0000-7000-8000-0000000000e3', 'Daily links',
                     'script:daily-links.sh', 'script', 1, 'review', 0, 3600, '0 6 * * *',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&mut connection)
        .await
        .expect("insert a subscription on the 0098 schema");
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    let subscriptions = database.list_subscriptions().await.expect("subscriptions");
    assert_eq!(subscriptions.len(), 1);
    assert!(subscriptions[0].script_arguments.is_empty());
    assert_eq!(subscriptions[0].schedule.as_deref(), Some("0 6 * * *"));
}

/// An indexer subscription stored before RD-180-20 sends no search term on upgrade.
///
/// Migration `0112` gives every row the empty search, so a subscription polls exactly what it
/// polled before; and the new `indexers` table starts empty.
#[tokio::test]
async fn an_indexer_subscription_from_before_the_search_fields_sends_none() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 111)
        .await
        .expect("schema at 0111");
    {
        let url = format!("sqlite://{}", path.display());
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        sqlx::query(
            "INSERT INTO subscriptions (id, name, url, kind, enabled, mode, priority,
                                        interval_seconds, created_at, updated_at)
             VALUES ('019d0000-0000-7000-8000-0000000000e4', 'Old indexer',
                     'https://indexer.example.test/api?t=tvsearch&q=kept', 'indexer', 1,
                     'review', 0, 3600, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&mut connection)
        .await
        .expect("insert a subscription on the 0111 schema");
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    let subscriptions = database.list_subscriptions().await.expect("subscriptions");
    assert_eq!(subscriptions.len(), 1);
    assert!(subscriptions[0].indexer_search.is_empty());
    assert!(database.list_indexers().await.expect("indexers").is_empty());
}

/// An indexer stored before RD-190-16 keeps the compact list on upgrade.
///
/// Migration `0114` gives every row `compact`, which is what every indexer's hits looked like
/// before the choice existed. Seeded on the `0112` schema, the last one on this branch below
/// `0114`; `0113` belongs to a parallel branch and does not touch `indexers`.
#[tokio::test]
async fn an_indexer_from_before_the_list_style_stays_compact() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 112)
        .await
        .expect("schema at 0112");
    {
        let url = format!("sqlite://{}", path.display());
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        sqlx::query(
            "INSERT INTO indexers (id, name, url, secret_ref, categories_json, enabled,
                                   created_at, updated_at)
             VALUES ('019d0000-0000-7000-8000-0000000000e5', 'Old indexer',
                     'https://indexer.example.test/api', NULL, '[]', 1,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&mut connection)
        .await
        .expect("insert an indexer on the 0112 schema");
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    let indexers = database.list_indexers().await.expect("indexers");
    assert_eq!(indexers.len(), 1);
    assert_eq!(indexers[0].name, "Old indexer");
    assert_eq!(indexers[0].list_style, rd_core::IndexerListStyle::Compact);
}

/// A subscription stored before RD-190-13 reads its git-release options as the empty choice.
///
/// Migration `0115` gives every row the empty object, which no kind but `git_release` reads.
#[tokio::test]
async fn a_subscription_from_before_the_git_release_options_reads_them_empty() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 112)
        .await
        .expect("schema at 0112");
    {
        let url = format!("sqlite://{}", path.display());
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        sqlx::query(
            "INSERT INTO subscriptions (id, name, url, kind, enabled, mode, priority,
                                        interval_seconds, created_at, updated_at)
             VALUES ('019d0000-0000-7000-8000-0000000000e5', 'Old feed',
                     'https://example.test/feed.xml', 'feed', 1,
                     'review', 0, 3600, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&mut connection)
        .await
        .expect("insert a subscription on the 0112 schema");
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    let subscriptions = database.list_subscriptions().await.expect("subscriptions");
    assert_eq!(subscriptions.len(), 1);
    assert!(subscriptions[0].git_release.is_empty());
}

/// RD-130-07: migration `0095` merges `comics` and `magazines` into `ebooks`, leaves `graphics`
/// alone, and lets go of the compiled-in pack's switches.
///
/// Seeded on the `0093` schema, the last one on this branch below `0095`; `0094` belongs to a
/// parallel branch and touches neither table.
#[tokio::test]
async fn the_site_rule_groups_are_merged_into_ebooks_and_off_wins() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 93)
        .await
        .expect("schema at 0093");
    {
        let url = format!("sqlite://{}", path.display());
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        for (id, group) in [
            ("old-magazine", "magazines"),
            ("old-comic", "comics"),
            ("old-graphics", "graphics"),
            ("old-board", "board"),
        ] {
            sqlx::query(
                "INSERT INTO site_rules (id, name, rule_group, enabled, rule_json, created_at,
                                         updated_at)
                 VALUES (?1, ?1, ?2, 1, json_object('id', ?1, 'group', ?2),
                         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            )
            .bind(id)
            .bind(group)
            .execute(&mut connection)
            .await
            .expect("insert a site rule on the 0093 schema");
        }
        for (scope, key, enabled) in [
            ("rule", "scnlog", 0),
            ("group", "magazines", 0),
            ("group", "ebooks", 1),
            ("group", "board", 0),
            ("group", "graphics", 0),
        ] {
            sqlx::query(
                "INSERT INTO site_rule_switches (scope, key, enabled, updated_at)
                 VALUES (?1, ?2, ?3, '2026-01-01T00:00:00Z')",
            )
            .bind(scope)
            .bind(key)
            .bind(enabled)
            .execute(&mut connection)
            .await
            .expect("insert a switch on the 0093 schema");
        }
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    let rules = database.list_site_rules().await.expect("site rules");
    for rule in &rules {
        let expected = match rule.id.as_str() {
            "old-board" => "board",
            "old-graphics" => "graphics",
            _ => "ebooks",
        };
        assert_eq!(rule.group, expected, "{}: the column", rule.id);
        assert_eq!(rule.rule["group"], expected, "{}: the body", rule.id);
    }
    assert_eq!(rules.len(), 4);

    let mut switches: Vec<(String, String, bool)> = database
        .list_site_rule_switches()
        .await
        .expect("switches")
        .into_iter()
        .map(|row| (row.scope, row.key, row.enabled))
        .collect();
    switches.sort();
    assert_eq!(
        switches,
        [
            ("group".to_owned(), "board".to_owned(), false),
            // `ebooks` was on and `magazines` off: the merged group is off.
            ("group".to_owned(), "ebooks".to_owned(), false),
            // `graphics` is a group of its own and keeps its switch.
            ("group".to_owned(), "graphics".to_owned(), false),
        ],
        "the merged groups' rows and the compiled-in pack's rule rows are gone"
    );
}

/// The owner's upgrade to 1.9 (RD-190-04): a package stored before migration `0113` keeps its
/// archive password through the upgrade, the first start moves it into the vault, and neither
/// the database file nor the copy the upgrade wrote beside it holds it afterwards.
#[tokio::test]
async fn an_archive_password_from_before_the_vault_survives_the_upgrade_and_leaves_the_files() {
    const PASSWORD: &str = "rd190-from-the-0111-schema";
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 111)
        .await
        .expect("schema at 0111");
    {
        let url = format!("sqlite://{}", path.display());
        let mut connection = SqliteConnection::connect(&url).await.expect("connect");
        sqlx::query(
            "INSERT INTO packages (id, name, state, destination, priority, position, kind,
                                   password, created_at, updated_at)
             VALUES ('019d0000-0000-7000-8000-0000000000f1', 'Protected', 'queued', 'downloads',
                     0, 1, 'http', ?, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .bind(PASSWORD)
        .execute(&mut connection)
        .await
        .expect("insert a package on the 0111 schema");
        connection.close().await.expect("close");
    }

    let database = rd_db::Database::open(&path).await.expect("upgrade");
    database
        .install_file_vault(directory.path().join("secrets"))
        .await
        .expect("vault");
    let copies = directory.path().join(rd_db::pre_migration::DIRECTORY);
    let copy_holds_it = || {
        std::fs::read_dir(&copies)
            .expect("the upgrade wrote a copy")
            .filter_map(Result::ok)
            .any(|entry| {
                std::fs::read(entry.path()).is_ok_and(|bytes| {
                    bytes
                        .windows(PASSWORD.len())
                        .any(|window| window == PASSWORD.as_bytes())
                })
            })
    };
    assert!(
        copy_holds_it(),
        "the copy before the upgrade is the old file"
    );

    assert_eq!(
        database
            .take_over_archive_passwords()
            .await
            .expect("takeover"),
        1
    );
    assert_eq!(
        rd_db::pre_migration::scrub_archive_passwords(&copies).await,
        1
    );

    let packages = database
        .list_packages_with_passwords()
        .await
        .expect("packages");
    assert_eq!(packages[0].password.as_deref(), Some(PASSWORD));
    database.checkpoint_wal().await.expect("checkpoint");
    let file = std::fs::read(&path).expect("database file");
    assert!(
        !file
            .windows(PASSWORD.len())
            .any(|window| window == PASSWORD.as_bytes()),
        "the database file still holds the password"
    );
    assert!(!copy_holds_it(), "the copy still holds the password");
}

/// DB-10: migration `0118` indexes six columns that were looked up or cascaded on without one,
/// and the per-download delete from the content index uses its index rather than a scan.
#[tokio::test]
async fn the_audit_indexes_exist_after_the_upgrade_and_carry_the_lookups() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 117)
        .await
        .expect("schema at 0117");
    drop(rd_db::Database::open(&path).await.expect("upgrade"));

    let url = format!("sqlite://{}", path.display());
    let mut connection = SqliteConnection::connect(&url).await.expect("connect");
    let mut indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name IN \
         ('content_index_path_idx', 'notification_deliveries_rule_idx', \
          'collector_packages_batch_idx', 'automation_runs_version_idx', \
          'plugin_repository_installs_repository_idx', 'object_uploads_profile_idx')",
    )
    .fetch_all(&mut connection)
    .await
    .expect("indexes");
    indexes.sort();
    assert_eq!(indexes.len(), 6, "{indexes:?}");
    let plan: Vec<(i64, i64, i64, String)> =
        sqlx::query_as("EXPLAIN QUERY PLAN DELETE FROM content_index WHERE path = 'x'")
            .fetch_all(&mut connection)
            .await
            .expect("plan");
    assert!(
        plan.iter()
            .any(|(_, _, _, detail)| detail.contains("content_index_path_idx")),
        "{plan:?}"
    );
    connection.close().await.expect("close");
}

/// RD-191-12: migration `0119` gives every download its limit-wait and automatic-retry counters,
/// both at zero, so a download queued before the upgrade starts with full budgets.
#[tokio::test]
async fn the_retry_counters_exist_after_the_upgrade_and_start_at_zero() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = schema_at(directory.path(), 118)
        .await
        .expect("schema at 0118");
    drop(rd_db::Database::open(&path).await.expect("upgrade"));

    let url = format!("sqlite://{}", path.display());
    let mut connection = SqliteConnection::connect(&url).await.expect("connect");
    let mut columns: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT name, \"notnull\", dflt_value FROM pragma_table_info('downloads') \
         WHERE name IN ('limit_waits', 'auto_retry_rounds')",
    )
    .fetch_all(&mut connection)
    .await
    .expect("columns");
    columns.sort();
    assert_eq!(
        columns,
        [
            ("auto_retry_rounds".to_owned(), 1, Some("0".to_owned())),
            ("limit_waits".to_owned(), 1, Some("0".to_owned())),
        ]
    );
}
