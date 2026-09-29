//! The release fixture: what every baseline is seeded with, and what it must look like after.
//!
//! One fixture for every baseline, written in the schema of the baseline it goes into. The base
//! rows use only columns the oldest baseline (`0033`) already carried. Where a later release
//! wrote a row differently — a column it had, a repair it had already made — a version gate
//! writes it the way that release did. The expectations are therefore the same for every
//! baseline, which is the point: a migration that rewrites old rows must arrive exactly where a
//! newer release's own rows already are.
//!
//! The rows a migration after `0033` rewrites, and so the ones this fixture carries:
//!
//! | Migration | Rewrites |
//! | --- | --- |
//! | `0048` | `completed_at` of finished packages, from their last write |
//! | `0052` | several default storage roots to one |
//! | `0067` | the recovery flag of queued PAR2 volumes |
//! | `0069` | extraction steps whose code sat inside the message |
//! | `0070` | no default category to one |
//! | `0074` | the LinkGrabber's two orders into one |
//! | `0095` | the `comics` site-rule group into `ebooks` |
//! | `0097` | inserts the official plugin repository |

use rd_core::{DownloadKind, DownloadState, PackageState};
use sqlx::SqliteConnection;

mod upgraded;

pub(crate) use upgraded::assert_upgraded;

/// `0048`: finished packages carry the time they finished.
const PACKAGE_COMPLETED_AT: i64 = 48;
/// `0052`: exactly one storage root is the default.
const SINGLE_DEFAULT_ROOT: i64 = 52;
/// `0067`: PAR2 volumes are marked as recovery data on the download row.
const DOWNLOAD_RECOVERY: i64 = 67;
/// `0069`: an extraction step's code has its own column.
const STEP_CODES: i64 = 69;
/// `0070`: exactly one category is the default.
const SINGLE_DEFAULT_CATEGORY: i64 = 70;
/// `0074`: collector packages and NZB imports share one order.
const GRABBER_SHARED_ORDER: i64 = 74;
/// `0076`: the site-rule table exists.
const SITE_RULES: i64 = 76;
/// `0095`: `comics` and `magazines` are `ebooks`.
const SITE_RULE_GROUPS_MERGED: i64 = 95;

const CREATED: &str = "2026-01-01T00:00:00Z";
/// When the completed package finished; its last write, too.
const FINISHED: &str = "2026-01-02T00:00:00Z";

const ROOT_ARCHIVE: &str = "019d0000-0000-7000-8000-0000000000f1";
const ROOT_DOWNLOADS: &str = "019d0000-0000-7000-8000-0000000000f2";
const CATEGORY_BOOKS: &str = "019d0000-0000-7000-8000-0000000000ca";
const CATEGORY_MOVIES: &str = "019d0000-0000-7000-8000-0000000000cb";
const ACCOUNT: &str = "019d0000-0000-7000-8000-0000000000ac";
const ACCOUNT_SECRET_REF: &str = "account/019d0000-0000-7000-8000-0000000000ac/secret";
const PACKAGE_QUEUED: &str = "019d0000-0000-7000-8000-0000000000a1";
const PACKAGE_COMPLETED: &str = "019d0000-0000-7000-8000-0000000000a2";
const PACKAGE_USENET: &str = "019d0000-0000-7000-8000-0000000000a3";
const CHUNK: &str = "019d0000-0000-7000-8000-0000000000c1";
const GRABBER_BATCH: &str = "019d0000-0000-7000-8000-0000000000b0";
const GRABBER_FIRST: &str = "019d0000-0000-7000-8000-0000000000b1";
const GRABBER_LAST: &str = "019d0000-0000-7000-8000-0000000000b2";
const GRABBER_NZB: &str = "019d0000-0000-7000-8000-0000000000ee";
const SITE_RULE: &str = "fixture-comic";

pub(crate) struct Package {
    id: &'static str,
    name: &'static str,
    state: PackageState,
    kind: DownloadKind,
    category: Option<&'static str>,
    updated_at: &'static str,
}

pub(crate) const PACKAGES: &[Package] = &[
    Package {
        id: PACKAGE_QUEUED,
        name: "Old package",
        state: PackageState::Queued,
        kind: DownloadKind::Http,
        category: Some(CATEGORY_MOVIES),
        updated_at: CREATED,
    },
    Package {
        id: PACKAGE_COMPLETED,
        name: "Finished season",
        state: PackageState::Completed,
        kind: DownloadKind::Http,
        category: None,
        updated_at: FINISHED,
    },
    Package {
        id: PACKAGE_USENET,
        name: "Usenet release",
        state: PackageState::Downloading,
        kind: DownloadKind::Usenet,
        category: None,
        updated_at: CREATED,
    },
];

pub(crate) struct Download {
    pub(crate) id: &'static str,
    package: &'static str,
    pub(crate) file_name: &'static str,
    state: DownloadState,
    kind: DownloadKind,
    total: Option<u64>,
    pub(crate) committed: u64,
    recovery: bool,
}

pub(crate) const DOWNLOADS: &[Download] = &[
    // The one the chunk checkpoint belongs to.
    Download {
        id: "019d0000-0000-7000-8000-0000000000d1",
        package: PACKAGE_QUEUED,
        file_name: "old.bin",
        state: DownloadState::Queued,
        kind: DownloadKind::Http,
        total: Some(4096),
        committed: 1024,
        recovery: false,
    },
    Download {
        id: "019d0000-0000-7000-8000-0000000000d2",
        package: PACKAGE_QUEUED,
        file_name: "paused.bin",
        state: DownloadState::Paused,
        kind: DownloadKind::Http,
        total: Some(8192),
        committed: 2048,
        recovery: false,
    },
    Download {
        id: "019d0000-0000-7000-8000-0000000000d3",
        package: PACKAGE_QUEUED,
        file_name: "gone.bin",
        state: DownloadState::Failed,
        kind: DownloadKind::Http,
        total: None,
        committed: 0,
        recovery: false,
    },
    Download {
        id: "019d0000-0000-7000-8000-0000000000d4",
        package: PACKAGE_COMPLETED,
        file_name: "episode.mkv",
        state: DownloadState::Completed,
        kind: DownloadKind::Http,
        total: Some(1000),
        committed: 1000,
        recovery: false,
    },
    Download {
        id: "019d0000-0000-7000-8000-0000000000d5",
        package: PACKAGE_USENET,
        file_name: "Release.vol00+01.par2",
        state: DownloadState::Queued,
        kind: DownloadKind::Usenet,
        total: Some(3000),
        committed: 0,
        recovery: true,
    },
    // Stopped mid-transfer, as a crash leaves it.
    Download {
        id: "019d0000-0000-7000-8000-0000000000d6",
        package: PACKAGE_USENET,
        file_name: "Release.part01.rar",
        state: DownloadState::Downloading,
        kind: DownloadKind::Usenet,
        total: Some(5000),
        committed: 2500,
        recovery: false,
    },
];

fn source_url(file_name: &str) -> String {
    format!("https://files.example.test/{file_name}")
}

/// The name a state or kind has in its column.
fn sql_name(value: impl serde::Serialize) -> anyhow::Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("a value that serialises as a string"))
}

/// Seeds the fixture into a database that stopped at migration `version`.
pub(crate) async fn seed(connection: &mut SqliteConnection, version: i64) -> anyhow::Result<()> {
    // Before `0052` nothing stopped two roots from both being the default.
    for (id, name, is_default) in [
        (ROOT_ARCHIVE, "Archive", true),
        (ROOT_DOWNLOADS, "Downloads", version < SINGLE_DEFAULT_ROOT),
    ] {
        sqlx::query(
            "INSERT INTO storage_roots (id, name, path, is_default, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        )
        .bind(id)
        .bind(name)
        .bind(format!("/srv/{}", name.to_lowercase()))
        .bind(is_default)
        .bind(CREATED)
        .execute(&mut *connection)
        .await?;
    }
    // Before `0070` nothing stopped an install from having no default category at all.
    for (id, name, is_default) in [
        (CATEGORY_BOOKS, "Books", version >= SINGLE_DEFAULT_CATEGORY),
        (CATEGORY_MOVIES, "Movies", false),
    ] {
        sqlx::query(
            "INSERT INTO categories (id, name, color, storage_root_id, relative_path, is_default,
                                     created_at, updated_at)
             VALUES (?1, ?2, '#38BDF8', ?3, ?4, ?5, ?6, ?6)",
        )
        .bind(id)
        .bind(name)
        .bind(ROOT_DOWNLOADS)
        .bind(name.to_lowercase())
        .bind(is_default)
        .bind(CREATED)
        .execute(&mut *connection)
        .await?;
    }
    sqlx::query(
        "INSERT INTO accounts (id, provider, label, username, secret_ref, enabled, created_at,
                               updated_at)
         VALUES (?1, 'realdebrid', 'Main account', 'someone', ?2, 1, ?3, ?3)",
    )
    .bind(ACCOUNT)
    .bind(ACCOUNT_SECRET_REF)
    .bind(CREATED)
    .execute(&mut *connection)
    .await?;

    seed_queue(connection, version).await?;
    seed_grabber(connection, version).await?;

    if version >= SITE_RULES {
        let group = if version < SITE_RULE_GROUPS_MERGED {
            "comics"
        } else {
            "ebooks"
        };
        sqlx::query(
            "INSERT INTO site_rules (id, name, rule_group, enabled, rule_json, created_at,
                                     updated_at)
             VALUES (?1, ?1, ?2, 1, json_object('id', ?1, 'group', ?2), ?3, ?3)",
        )
        .bind(SITE_RULE)
        .bind(group)
        .bind(CREATED)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

/// Packages, downloads, the chunk checkpoint and the failed extraction step.
async fn seed_queue(connection: &mut SqliteConnection, version: i64) -> anyhow::Result<()> {
    for (position, package) in (1_i64..).zip(PACKAGES) {
        sqlx::query(
            "INSERT INTO packages (id, name, state, destination, category_id, priority, position,
                                   kind, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'downloads', ?4, 0, ?5, ?6, ?7, ?8)",
        )
        .bind(package.id)
        .bind(package.name)
        .bind(sql_name(package.state)?)
        .bind(package.category)
        .bind(position)
        .bind(sql_name(package.kind)?)
        .bind(CREATED)
        .bind(package.updated_at)
        .execute(&mut *connection)
        .await?;
    }
    if version >= PACKAGE_COMPLETED_AT {
        sqlx::query("UPDATE packages SET completed_at = ?1 WHERE id = ?2")
            .bind(FINISHED)
            .bind(PACKAGE_COMPLETED)
            .execute(&mut *connection)
            .await?;
    }

    for (position, download) in (1_i64..).zip(DOWNLOADS) {
        sqlx::query(
            "INSERT INTO downloads (id, package_id, source_url, file_name, state, total_bytes,
                                    committed_bytes, kind, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        )
        .bind(download.id)
        .bind(download.package)
        .bind(source_url(download.file_name))
        .bind(download.file_name)
        .bind(sql_name(download.state)?)
        .bind(download.total.map(i64::try_from).transpose()?)
        .bind(i64::try_from(download.committed)?)
        .bind(sql_name(download.kind)?)
        .bind(position)
        .bind(CREATED)
        .execute(&mut *connection)
        .await?;
        // From `0067` on the release decided this itself when it queued the NZB.
        if version >= DOWNLOAD_RECOVERY && download.recovery {
            sqlx::query("UPDATE downloads SET recovery = 1 WHERE id = ?1")
                .bind(download.id)
                .execute(&mut *connection)
                .await?;
        }
    }
    sqlx::query(
        "INSERT INTO chunks (id, download_id, start_offset, end_offset, committed_offset,
                             updated_at)
         VALUES (?1, ?2, 0, 4096, 1024, ?3)",
    )
    .bind(CHUNK)
    .bind(DOWNLOADS[0].id)
    .bind(CREATED)
    .execute(&mut *connection)
    .await?;

    // The shape 1.0.x wrote before `0069`: the stable code as the message's first word.
    sqlx::query(
        "INSERT INTO postprocess_steps (owner_id, kind, source_path, state, message, updated_at)
         VALUES (?1, 'extract_rar', '/srv/downloads/season/Release.part01.rar', 'failed',
                 'extract.data_damaged Release.part01.rar', ?2)",
    )
    .bind(PACKAGE_COMPLETED)
    .bind(FINISHED)
    .execute(&mut *connection)
    .await?;
    if version >= STEP_CODES {
        sqlx::query(
            "UPDATE postprocess_steps
                SET code = 'extract.data_damaged', params_json = '{\"detail\":\"Release.part01.rar\"}',
                    message = 'Release.part01.rar'
              WHERE owner_id = ?1",
        )
        .bind(PACKAGE_COMPLETED)
        .execute(&mut *connection)
        .await?;
    }
    Ok(())
}

/// Two collector packages and an NZB import created between them.
///
/// Before `0074` the import had no position: the list slotted it in by creation time, between
/// the two packages, and that is where the backfill must put it.
async fn seed_grabber(connection: &mut SqliteConnection, version: i64) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO collector_batches (id, source, created_at) VALUES (?1, 'clipboard', ?2)",
    )
    .bind(GRABBER_BATCH)
    .bind(CREATED)
    .execute(&mut *connection)
    .await?;
    for (id, name, position, created_at) in [
        (
            GRABBER_FIRST,
            "Grabbed first",
            1_i64,
            "2026-01-01T00:00:01Z",
        ),
        (GRABBER_LAST, "Grabbed last", 2, "2026-01-01T00:00:03Z"),
    ] {
        sqlx::query(
            "INSERT INTO collector_packages (id, batch_id, name, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        )
        .bind(id)
        .bind(GRABBER_BATCH)
        .bind(name)
        .bind(position)
        .bind(created_at)
        .execute(&mut *connection)
        .await?;
    }
    sqlx::query(
        "INSERT INTO nzb_imports (id, name, sha256, state, file_count, segment_count, total_bytes,
                                  created_at, updated_at)
         VALUES (?1, 'Grabbed.nzb', ?2, 'imported', 0, 0, 0, '2026-01-01T00:00:02Z',
                 '2026-01-01T00:00:02Z')",
    )
    .bind(GRABBER_NZB)
    .bind("0".repeat(64))
    .execute(&mut *connection)
    .await?;
    if version >= GRABBER_SHARED_ORDER {
        sqlx::query("UPDATE collector_packages SET position = 3 WHERE id = ?1")
            .bind(GRABBER_LAST)
            .execute(&mut *connection)
            .await?;
        sqlx::query("UPDATE nzb_imports SET position = 2 WHERE id = ?1")
            .bind(GRABBER_NZB)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}
