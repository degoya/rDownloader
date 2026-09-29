//! Every release baseline, seeded with the release fixture and upgraded to today's schema.

use crate::{BASELINES, PENDING_GAPS, RESERVED_GAPS, fixture, seeded_at};

/// The fixture of an older installation survives the upgrade, byte counts included.
#[tokio::test]
async fn every_release_baseline_upgrades_with_its_queue_intact() {
    for (release, version) in BASELINES {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = seeded_at(directory.path(), *version)
            .await
            .unwrap_or_else(|error| panic!("{release}: seed at migration {version}: {error}"));

        // Opening is what applies the remaining migrations, exactly as a restart would.
        let database = rd_db::Database::open(&path)
            .await
            .unwrap_or_else(|error| panic!("{release}: upgrade from migration {version}: {error}"));

        fixture::assert_upgraded(&database, &path, release, *version).await;
    }
}

/// An upgraded database is writable, not merely readable.
///
/// A migration can leave a schema that reads back correctly and rejects the next insert — a
/// tightened constraint, a column that became `NOT NULL` without a default. Reading alone
/// would not notice until the first real download.
#[tokio::test]
async fn an_upgraded_database_still_accepts_writes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = seeded_at(directory.path(), BASELINES[0].1)
        .await
        .expect("seed");
    let database = rd_db::Database::open(&path).await.expect("upgrade");

    let package = database
        .create_package(rd_db::NewPackage {
            id: rd_core::PackageId::new(),
            name: "After the upgrade".to_owned(),
            destination: "downloads".to_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::default(),
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("create a package on the upgraded schema");
    assert_eq!(
        database.list_packages().await.expect("packages").len(),
        fixture::PACKAGES.len() + 1
    );
    assert_eq!(package.name, "After the upgrade");
}

/// Startup recovery runs against an upgraded database without losing the checkpoint.
///
/// `recover_interrupted` resets active states to queued, and it is the first thing that
/// touches an upgraded database on a real start. A checkpoint it discarded would be bytes
/// re-downloaded on every upgrade.
#[tokio::test]
async fn recovery_after_an_upgrade_keeps_the_checkpoint() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = seeded_at(directory.path(), BASELINES[1].1)
        .await
        .expect("seed");
    let database = rd_db::Database::open(&path).await.expect("upgrade");

    database.recover_interrupted().await.expect("recover");
    let checkpointed = &fixture::DOWNLOADS[0];
    let downloads = database.list_downloads().await.expect("downloads");
    let download = downloads
        .iter()
        .find(|download| download.id.to_string() == checkpointed.id)
        .expect("the checkpointed download");
    assert_eq!(
        download.committed_bytes.get(),
        checkpointed.committed,
        "recovery discarded a checkpoint that the file on disk still matches"
    );
}

/// The reserved numbers really are absent, and nothing else is.
///
/// Without this the tolerance in `migrator_up_to` would hide a genuinely missing migration: a
/// chain that skipped 0080 by accident would simply be declared to have another reserved gap.
#[test]
fn only_the_reserved_numbers_are_missing_from_the_chain() {
    let migrations = sqlx::migrate!();
    let present: Vec<i64> = migrations
        .migrations
        .iter()
        .map(|migration| migration.version)
        .collect();
    let highest = *present.last().expect("at least one migration");
    let missing: Vec<i64> = (1..=highest)
        .filter(|version| !present.contains(version))
        .collect();
    let mut expected: Vec<i64> = RESERVED_GAPS.iter().chain(PENDING_GAPS).copied().collect();
    expected.sort_unstable();
    assert_eq!(
        missing, expected,
        "a migration number is missing that is neither reserved nor held by a parallel branch"
    );
    let baseline = BASELINES.last().map_or(0, |(_, version)| *version);
    assert!(
        PENDING_GAPS.iter().all(|gap| *gap > baseline),
        "a pending gap below a release baseline would silently change that release's count"
    );
}
