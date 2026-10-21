//! The two release baselines, seeded with the release fixture and upgraded to today's schema,
//! and the guards of the ratchet (RD-1280-23).

use super::{BASELINES, PENDING_GAPS, RESERVED_GAPS, fixture, seeded_at};

/// The fixture of an older installation survives the upgrade, byte counts included.
async fn upgrades_with_its_queue_intact(release: &str) {
    let version = BASELINES
        .iter()
        .find_map(|(name, version)| (*name == release).then_some(*version))
        .unwrap_or_else(|| panic!("{release} is no row of BASELINES"));
    let directory = tempfile::tempdir().expect("tempdir");
    let path = seeded_at(directory.path(), version)
        .await
        .unwrap_or_else(|error| panic!("{release}: seed at migration {version}: {error}"));

    // Opening is what applies the remaining migrations, exactly as a restart would.
    let database = rd_db::Database::open(&path)
        .await
        .unwrap_or_else(|error| panic!("{release}: upgrade from migration {version}: {error}"));

    fixture::assert_upgraded(&database, &path, release, version).await;
}

/// One test per release baseline (RD-1120-08): they run in parallel and each stays short, where
/// one test over all of them grew with baselines times migrations towards nextest's limit.
macro_rules! every_release_baseline {
    ($($test:ident => $release:literal,)+) => {
        $(
            #[tokio::test]
            async fn $test() {
                upgrades_with_its_queue_intact($release).await;
            }
        )+

        /// The releases the tests above upgrade from.
        const UPGRADED: &[&str] = &[$($release),+];
    };
}

every_release_baseline! {
    the_1_34_0_baseline_upgrades_with_its_queue_intact => "1.34.0",
    the_1_35_0_baseline_upgrades_with_its_queue_intact => "1.35.0",
}

/// A row added to BASELINES gets its test above; without this it would upgrade nowhere.
#[test]
fn every_release_baseline_has_its_upgrade_test() {
    let releases: Vec<&str> = BASELINES.iter().map(|(release, _)| *release).collect();
    assert_eq!(UPGRADED, releases.as_slice());
}

/// `major.minor.patch` of a release; `None` for a pre-release, which keeps the baselines.
fn release_number(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| part.parse::<u64>().ok());
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) => {
            Some((major, minor, patch))
        }
        _ => None,
    }
}

/// The ratchet holds: at most two rows, one per minor version, oldest first, and the newest is
/// the release this tree is — `RD_RELEASE_VERSION` while the release chain checks the tree before
/// its version bump, the workspace version otherwise. A release that skipped
/// `scripts/migration-baselines.sh` fails here on the next check.
#[test]
fn the_baselines_are_the_last_two_releases() {
    assert!(
        (1..=2).contains(&BASELINES.len()),
        "the suite keeps the last two releases: {BASELINES:?}"
    );
    let numbers: Vec<(u64, u64, u64)> = BASELINES
        .iter()
        .map(|(release, _)| release_number(release).expect("a release row is X.Y.Z"))
        .collect();
    for pair in numbers.windows(2) {
        assert!(
            (pair[0].0, pair[0].1) < (pair[1].0, pair[1].1),
            "one row per minor version, oldest first: {BASELINES:?}"
        );
    }
    for pair in BASELINES.windows(2) {
        assert!(
            pair[0].1 <= pair[1].1,
            "a newer release shipped fewer migrations: {BASELINES:?}"
        );
    }
    let releasing = std::env::var("RD_RELEASE_VERSION").ok();
    let current = releasing
        .as_deref()
        .filter(|version| !version.is_empty())
        .unwrap_or(env!("CARGO_PKG_VERSION"));
    if release_number(current).is_some() {
        let newest = BASELINES.last().map(|(release, _)| *release);
        assert_eq!(
            newest,
            Some(current),
            "the newest baseline is the release this tree is; run scripts/migration-baselines.sh {current}"
        );
    }
}

/// The schema a per-migration test starts from, read off its source: every
/// `schema_at(directory.path(), N)` in `per_migration.rs`.
fn measured_schemas(source: &str) -> Vec<i64> {
    source
        .split("schema_at(directory.path(), ")
        .skip(1)
        .map(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().expect("a migration number after schema_at")
        })
        .collect()
}

/// A per-migration test measures a migration the older kept release did not ship: its schema
/// is at least that release's last migration. One the release script's drop left behind fails
/// here, naming the number to delete it by.
#[test]
fn every_measured_migration_is_newer_than_the_older_baseline() {
    let oldest = BASELINES[0].1;
    for schema in measured_schemas(include_str!("per_migration.rs")) {
        assert!(
            schema >= oldest,
            "per_migration.rs measures migration {:04}, which both kept releases ship; its test goes",
            schema + 1
        );
    }
    assert_eq!(
        measured_schemas("schema_at(directory.path(), 145)\nschema_at(directory.path(), 9)"),
        [145, 9],
        "the reading of the source itself"
    );
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
