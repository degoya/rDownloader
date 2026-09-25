//! An applied migration stays as it is (RD-120-41).
//!
//! sqlx stores the SHA-384 of every migration file it applies, in `_sqlx_migrations.checksum`,
//! and compares it on every start. One changed byte -- a comment is enough -- and every
//! installation that applied the file refuses to start with "migration N was previously applied
//! but has been modified". That happened to the owner's instance on 2026-09-23 over one comment
//! line in `0065_remote_jobs.sql`. The rule had been prose in `AGENTS.md`; this file is the rule.
//!
//! `crates/rd-db/migrations.sha384` pins every migration's checksum, one line per number, in
//! plain `sha384sum` format. It is written only by `scripts/migration-pin.sh`, which appends and
//! never rewrites. The pins are what installations applied: `0059` and `0069` were edited once
//! long before this test existed and are pinned as they are today, and `0065` is pinned at the
//! original bytes it was restored to (`e89a4ad5`). On 2026-09-23 all 89 rows of the owner's
//! live `_sqlx_migrations` table matched the pins.

use std::{borrow::Cow, collections::BTreeMap};

use sqlx::{
    Connection, SqliteConnection,
    migrate::{Migration, MigrationType},
};

/// The pin file, compiled in so that editing it rebuilds this test.
const PINS: &str = include_str!("../migrations.sha384");

/// The one command that adds a pin.
const PIN_COMMAND: &str = "scripts/migration-pin.sh";

/// One line of the pin file.
struct Pin {
    checksum: String,
    file: String,
}

/// The pin file, keyed by migration number.
fn pins() -> BTreeMap<i64, Pin> {
    parse_pins(PINS)
}

/// Parses pin-file text, keyed by migration number.
fn parse_pins(text: &str) -> BTreeMap<i64, Pin> {
    let mut pins = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let (checksum, file) = line.split_once("  ").unwrap_or_else(|| {
            panic!("migrations.sha384 line {line_number} is not `<sha384>  <file>`: {line:?}")
        });
        assert!(
            checksum.len() == 96 && checksum.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "migrations.sha384 line {line_number} carries no SHA-384: {line:?}"
        );
        let version: i64 = file
            .split_once('_')
            .and_then(|(number, _)| number.parse().ok())
            .unwrap_or_else(|| {
                panic!("migrations.sha384 line {line_number} names no numbered file: {line:?}")
            });
        let previous = pins.insert(
            version,
            Pin {
                checksum: checksum.to_ascii_lowercase(),
                file: file.to_owned(),
            },
        );
        assert!(
            previous.is_none(),
            "migrations.sha384 pins migration {version} twice"
        );
    }
    pins
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The file name sqlx read the migration from; it derives the description by replacing `_`.
fn file_name(migration: &Migration) -> String {
    format!(
        "{:04}_{}.sql",
        migration.version,
        migration.description.replace(' ', "_")
    )
}

/// Everything that would stop an installation from starting, one message each.
fn problems<'a>(
    migrations: impl IntoIterator<Item = &'a Migration>,
    mut pins: BTreeMap<i64, Pin>,
) -> Vec<String> {
    let mut problems = Vec::new();

    for migration in migrations {
        let file = file_name(migration);
        let Some(pin) = pins.remove(&migration.version) else {
            problems.push(format!(
                "{file} has no pinned checksum. Pin it now, while no installation has applied it \
                 yet, and commit the pin with the migration:\n    {PIN_COMMAND} {file}"
            ));
            continue;
        };
        if pin.file != file {
            problems.push(format!(
                "migration {} is pinned as {} but the file is {file}. Keep the name it was \
                 pinned under.",
                migration.version, pin.file
            ));
        }
        let actual = hex(&migration.checksum);
        if pin.checksum != actual {
            problems.push(format!(
                "{file} changed after it was pinned (pinned {}, now {actual}). Every installation \
                 that applied it refuses to start: \"migration {} was previously applied but has \
                 been modified\" -- even over a comment. Restore the file's bytes \
                 (`git log -- crates/rd-db/migrations/{file}`) and put the change in a new \
                 migration. Never move the pin.",
                pin.checksum, migration.version
            ));
        }
    }
    for (version, pin) in pins {
        problems.push(format!(
            "{} is pinned but no longer exists. Every installation that applied it refuses to \
             start: \"migration {version} was previously applied but is missing in the resolved \
             migrations\". Restore the file.",
            pin.file
        ));
    }
    problems
}

/// Every migration has a pin, and every pin still matches the file sqlx compiles in.
///
/// The checksum compared here is the one `sqlx::migrate!()` computes -- the very value a start
/// writes into and compares against `_sqlx_migrations`.
#[test]
fn every_migration_is_pinned_and_unchanged() {
    let problems = problems(sqlx::migrate!().iter(), pins());
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n\n"));
}

/// A migration as the migrator would read it from `NNNN_<description>.sql`.
fn migration(version: i64, description: &'static str, sql: &'static str) -> Migration {
    Migration::new(
        version,
        Cow::Borrowed(description),
        MigrationType::Simple,
        Cow::Borrowed(sql),
        false,
    )
}

/// The line `scripts/migration-pin.sh` would write for a file holding `sql`.
fn pin_line(sql: &'static str, file: &str) -> String {
    format!("{}  {file}\n", hex(&migration(0, "", sql).checksum))
}

/// A new migration without a pin fails, and the message is the command that pins it.
///
/// Synthetic on purpose: proving this against the real tree means dropping a probe file into
/// `migrations/`, and an interrupted run leaves it there to be committed (it was, once).
#[test]
fn a_new_migration_without_a_pin_names_the_command_that_pins_it() {
    let first = migration(1, "initial", "CREATE TABLE a (id INTEGER);\n");
    let added = migration(2, "probe only", "SELECT 1;\n");
    let pins = parse_pins(&pin_line(
        "CREATE TABLE a (id INTEGER);\n",
        "0001_initial.sql",
    ));

    let problems = problems([&first, &added], pins);
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(
        problems[0].contains("\n    scripts/migration-pin.sh 0002_probe_only.sql"),
        "{}",
        problems[0]
    );
}

/// One changed byte in a comment fails, and the message points at a new migration.
#[test]
fn a_changed_comment_fails_and_points_at_a_new_migration() {
    let pins = parse_pins(&pin_line(
        "-- 'magnet' or 'container'\nSELECT 1;\n",
        "0065_remote_jobs.sql",
    ));
    let edited = migration(65, "remote jobs", "-- 'magnet' or 'address'\nSELECT 1;\n");

    let problems = problems([&edited], pins);
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(
        problems[0].contains("migration 65 was previously applied but has been modified")
            && problems[0].contains("new migration"),
        "{}",
        problems[0]
    );
}

/// A pinned migration that disappeared fails too; sqlx refuses a missing applied migration.
#[test]
fn a_removed_migration_fails() {
    let pins = parse_pins(&pin_line("SELECT 1;\n", "0007_gone.sql"));
    let problems = problems(std::iter::empty(), pins);
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(problems[0].contains("0007_gone.sql is pinned but no longer exists"));
}

/// The pins are the checksums sqlx really stores, not a formula assumed to match it.
///
/// Opens a fresh database through the same path a start takes, then reads `_sqlx_migrations`
/// back: the pins were written by `sha384sum` over the file bytes, so equality here proves the
/// script and sqlx agree.
#[tokio::test]
async fn the_pins_are_what_sqlx_stores_in_a_real_database() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("rdownloader.sqlite3");
    let database = rd_db::Database::open(&path).await.expect("open database");

    let url = format!("sqlite://{}", path.display());
    let mut connection = SqliteConnection::connect(&url).await.expect("connect");
    let stored: Vec<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut connection)
            .await
            .expect("read _sqlx_migrations");
    connection.close().await.expect("close");
    drop(database);

    let pins = pins();
    assert_eq!(
        stored
            .iter()
            .map(|(version, ..)| *version)
            .collect::<Vec<_>>(),
        pins.keys().copied().collect::<Vec<_>>(),
        "the pinned numbers are not the numbers sqlx applied"
    );
    for (version, checksum, success) in &stored {
        assert!(success, "migration {version} did not apply");
        assert_eq!(
            hex(checksum),
            pins[version].checksum,
            "{}: the pin is not what sqlx stored",
            pins[version].file
        );
    }
}
