//! Single migrations, each measured on a row written by the schema right before it.
//!
//! These start from an empty database at that schema rather than from the release fixture:
//! each asserts one table, and the fixture's own rows in it would only blur the count.

use sqlx::{Connection, SqliteConnection};

use crate::schema_at;

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
