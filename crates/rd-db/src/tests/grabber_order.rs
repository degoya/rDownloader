//! The one position sequence LinkGrabber packages and NZB imports share.

use rd_core::IngressSource;

use super::dropped_nzb;
use crate::Database;

/// One collector package holding one link, as a LinkGrabber entry of the "collector" kind.
async fn grabber_package(database: &Database, url: &str) -> rd_core::CollectorPackageId {
    let (_, packages, _) = database
        .add_collector_batch(crate::NewCollectorBatch {
            package_hints: Vec::new(),
            mirror_hints: Vec::new(),
            source: IngressSource::Manual,
            source_label: None,
            package_name: None,
            password: None,
            passwords: Vec::new(),
            category_id: None,
            priority: None,
            urls: vec![url.parse().expect("URL")],
            providers: vec![None],
            file_names: Vec::new(),
            sizes: Vec::new(),
            requests: Vec::new(),
            body_refs: Vec::new(),
            auto_check: false,
            source_attributes: Vec::new(),
        })
        .await
        .expect("batch");
    packages[0].id
}

fn collector_entry(id: rd_core::CollectorPackageId) -> rd_core::GrabberEntryRef {
    rd_core::GrabberEntryRef {
        kind: rd_core::GrabberEntryKind::Collector,
        id: id.into_uuid(),
    }
}

fn nzb_entry(id: rd_core::NzbImportId) -> rd_core::GrabberEntryRef {
    rd_core::GrabberEntryRef {
        kind: rd_core::GrabberEntryKind::Nzb,
        id: id.into_uuid(),
    }
}

/// Both kinds draw from one position sequence, so a mixed order survives a round trip.
///
/// The interleaving is the whole point: two independent sequences can only ever put one kind
/// before the other, which is why an NZB import could not be dragged between two packages.
#[tokio::test]
async fn a_mixed_linkgrabber_order_round_trips_through_one_position_sequence() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("grabber.sqlite"))
        .await
        .expect("database");
    let first_package = grabber_package(&database, "https://ddownload.com/first").await;
    let first_import = database
        .add_nzb_import(dropped_nzb(
            "first.nzb",
            &"a1".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("first import")
        .id;
    let second_package = grabber_package(&database, "https://ddownload.com/second").await;
    let second_import = database
        .add_nzb_import(dropped_nzb(
            "second.nzb",
            &"b2".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("second import")
        .id;

    database
        .reorder_grabber_entries(
            vec![
                nzb_entry(second_import),
                collector_entry(first_package),
                nzb_entry(first_import),
                collector_entry(second_package),
            ],
            None,
        )
        .await
        .expect("reorder");

    let packages = database
        .list_collector_packages()
        .await
        .expect("packages")
        .into_iter()
        .map(|package| (package.id, package.position))
        .collect::<Vec<_>>();
    let imports = database
        .list_nzb_imports()
        .await
        .expect("imports")
        .into_iter()
        .map(|import| (import.id, import.position))
        .collect::<Vec<_>>();
    assert_eq!(packages, vec![(first_package, 2), (second_package, 4)]);
    assert_eq!(imports, vec![(second_import, 1), (first_import, 3)]);
}

/// An entry that names no row is refused, and the order that was there stays untouched.
///
/// The refusal has to happen before anything is written: the unknown id updates no row and
/// reports success, so the remaining entries would silently take the positions of the list the
/// caller thought it was sending.
#[tokio::test]
async fn a_reorder_naming_an_unknown_entry_is_refused_and_writes_nothing() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("unknown.sqlite"))
        .await
        .expect("database");
    let package = grabber_package(&database, "https://ddownload.com/only").await;
    let import = database
        .add_nzb_import(dropped_nzb(
            "only.nzb",
            &"c3".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("import")
        .id;
    let before = (
        database.list_collector_packages().await.expect("packages")[0].position,
        database.list_nzb_imports().await.expect("imports")[0].position,
    );

    let error = database
        .reorder_grabber_entries(
            vec![
                nzb_entry(import),
                collector_entry(package),
                collector_entry(rd_core::CollectorPackageId::new()),
            ],
            None,
        )
        .await
        .expect_err("unknown entry");
    assert_eq!(
        crate::store_kind(&error),
        Some(crate::StoreErrorKind::NotFound)
    );

    // An id of the wrong kind is the same mistake by another route: both are UUIDs, so only the
    // declared kind says which table to look in.
    let wrong_kind = database
        .reorder_grabber_entries(
            vec![nzb_entry(rd_core::NzbImportId::from_uuid(
                package.into_uuid(),
            ))],
            None,
        )
        .await
        .expect_err("wrong kind");
    assert_eq!(
        crate::store_kind(&wrong_kind),
        Some(crate::StoreErrorKind::NotFound)
    );

    let after = (
        database.list_collector_packages().await.expect("packages")[0].position,
        database.list_nzb_imports().await.expect("imports")[0].position,
    );
    assert_eq!(before, after);
}

/// Upgrading to the shared sequence does not move anything the list was already showing.
///
/// Migration 0074 is the only place where existing rows of both tables are numbered against each
/// other, and getting it wrong reshuffles every LinkGrabber in the field exactly once. The two
/// cases are the two orders that existed before it: nobody had dragged, and somebody had.
#[tokio::test]
async fn the_shared_order_backfill_preserves_the_order_the_list_showed_before() {
    for dragged in [false, true] {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("backfill.sqlite");
        let mut connection = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .expect("connect");

        // The schema as an installation in the field has it: everything up to, but excluding,
        // the migration under test.
        let before = sqlx::migrate::Migrator {
            migrations: std::borrow::Cow::Owned(
                sqlx::migrate!()
                    .iter()
                    .filter(|migration| migration.version < 74)
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            ignore_missing: false,
            locking: true,
            no_tx: false,
            ..sqlx::migrate::Migrator::DEFAULT
        };
        before.run(&mut connection).await.expect("migrate to 0073");

        sqlx::query("INSERT INTO collector_batches (id, source, created_at) VALUES ('batch', 'manual', '2026-01-01T00:00:00Z')")
            .execute(&mut connection)
            .await
            .expect("batch");
        // Four entries, alternating by creation time: package, import, package, import.
        for (id, created_at, position) in [
            ("pkg-a", "2026-01-01T00:00:01Z", i64::from(dragged) * 2),
            ("pkg-b", "2026-01-01T00:00:03Z", i64::from(dragged)),
        ] {
            sqlx::query(
                "INSERT INTO collector_packages (id, batch_id, name, auto_named, priority, position, created_at, updated_at) \
                 VALUES (?, 'batch', ?, 0, 0, ?, ?, ?)",
            )
            .bind(id)
            .bind(id)
            .bind(position)
            .bind(created_at)
            .bind(created_at)
            .execute(&mut connection)
            .await
            .expect("package");
        }
        for (id, created_at) in [
            ("nzb-a", "2026-01-01T00:00:02Z"),
            ("nzb-b", "2026-01-01T00:00:04Z"),
        ] {
            sqlx::query(
                "INSERT INTO nzb_imports (id, name, sha256, state, file_count, segment_count, total_bytes, import_mode, created_at, updated_at) \
                 VALUES (?, ?, ?, 'imported', 0, 0, 0, 'review', ?, ?)",
            )
            .bind(id)
            .bind(id)
            .bind(id)
            .bind(created_at)
            .bind(created_at)
            .execute(&mut connection)
            .await
            .expect("import");
        }

        sqlx::migrate!()
            .run(&mut connection)
            .await
            .expect("migrate to 0074");

        let order: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM (SELECT id AS id, position AS position, created_at AS created_at FROM collector_packages \
             UNION ALL SELECT id, position, created_at FROM nzb_imports) ORDER BY position, created_at, id",
        )
        .fetch_all(&mut connection)
        .await
        .expect("order");
        let positions: Vec<i64> = sqlx::query_scalar(
            "SELECT position FROM (SELECT id AS id, position AS position, created_at AS created_at FROM collector_packages \
             UNION ALL SELECT id, position, created_at FROM nzb_imports) ORDER BY position, created_at, id",
        )
        .fetch_all(&mut connection)
        .await
        .expect("positions");

        assert_eq!(positions, vec![1, 2, 3, 4], "one gapless shared sequence");
        if dragged {
            // Spine after the drag: pkg-b, pkg-a. The client emitted nzb-a first (pkg-b is newer
            // than it) and nzb-b last (nothing in the spine is newer).
            assert_eq!(order, vec!["nzb-a", "pkg-b", "pkg-a", "nzb-b"]);
        } else {
            assert_eq!(order, vec!["pkg-a", "nzb-a", "pkg-b", "nzb-b"]);
        }
    }
}

/// A drag deep in the list names the moved row and the row it landed behind, nothing else.
///
/// Without the anchor a partial list could only describe a prefix, so moving the last of four
/// entries one step up meant sending three of them - and in a real LinkGrabber, hundreds. The
/// anchor is what keeps the request the same size wherever the row sits.
#[tokio::test]
async fn an_anchored_reorder_moves_one_entry_without_naming_the_rows_above_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("anchor.sqlite"))
        .await
        .expect("database");
    let first = grabber_package(&database, "https://ddownload.com/one").await;
    let second = grabber_package(&database, "https://ddownload.com/two").await;
    let third = grabber_package(&database, "https://ddownload.com/three").await;
    let import = database
        .add_nzb_import(dropped_nzb(
            "last.nzb",
            &"d4".repeat(32),
            None,
            IngressSource::Manual,
            None,
        ))
        .await
        .expect("import")
        .id;

    // The import is last; drop it behind the first package without mentioning the other two.
    database
        .reorder_grabber_entries(vec![nzb_entry(import)], Some(collector_entry(first)))
        .await
        .expect("anchored reorder");

    let packages = database
        .list_collector_packages()
        .await
        .expect("packages")
        .into_iter()
        .map(|package| (package.id, package.position))
        .collect::<Vec<_>>();
    let imports = database
        .list_nzb_imports()
        .await
        .expect("imports")
        .into_iter()
        .map(|item| (item.id, item.position))
        .collect::<Vec<_>>();
    assert_eq!(
        packages,
        vec![(first, 1), (second, 3), (third, 4)],
        "the untouched packages keep their relative order and close up behind the import"
    );
    assert_eq!(imports, vec![(import, 2)]);

    // An anchor that names no row is refused, exactly like an unknown entry: splicing behind
    // nothing would silently fall back to the head of the list.
    let error = database
        .reorder_grabber_entries(
            vec![nzb_entry(import)],
            Some(collector_entry(rd_core::CollectorPackageId::new())),
        )
        .await
        .expect_err("unknown anchor");
    assert_eq!(
        crate::store_kind(&error),
        Some(crate::StoreErrorKind::NotFound)
    );
}
