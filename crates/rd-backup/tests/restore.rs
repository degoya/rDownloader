//! Restoring a full backup (RD-160-03), from an archive an installation with Windows paths
//! wrote: the passphrase is asked for and a wrong one opens nothing, a preview reads everything
//! and changes nothing, a test restore migrates the copy and moves the Windows paths onto this
//! system, a stored `..` refuses the plan, and a staged restore switches at the next start with
//! the previous installation kept until the restored one started — or put back when it does
//! not.

mod common;

use std::path::Path;

use common::{
    INFO_HASH, PASSPHRASE, SHARE_PACKAGE, WINDOWS_PACKAGE, WINDOWS_ROOT, listing, target,
    windows_archive,
};
use rd_backup::manifest::{DATABASE_PART, PartKind, SETTINGS_PART};
use rd_backup::restore::cutover::{self, Cutover, Layout, PendingRestore, Phase};
use rd_backup::restore::inspect::{key_for, read_archive, unpack};
use rd_backup::restore::plan::{RequestedMapping, plan_paths, rewrite_session};
use rd_db::restore_copy::{self, CopyUpdate, PATH_COLUMNS, STORAGE_ROOT_PATH};
use tempfile::TempDir;

fn pending() -> PendingRestore {
    PendingRestore {
        phase: Phase::Staged,
        staged_at: chrono::Utc::now(),
        archive_name: "rdownloader-backup-test.rdbackup".to_owned(),
        backup_created_at: chrono::Utc::now(),
        app_version: "1.6.0-windows".to_owned(),
        minted_secrets: vec!["vault://minted".to_owned()],
    }
}

/// Unpacks the fixture into `into` and migrates its database copy.
async fn prepared(archive: &Path, into: &Path) {
    let key = key_for(archive, PASSPHRASE).await.expect("key");
    unpack(archive, key, into).await.expect("unpack");
    restore_copy::migrate_copy(&into.join(DATABASE_PART))
        .await
        .expect("migrate");
}

async fn root_paths(database: &Path) -> Vec<String> {
    restore_copy::read_cells(database, &[STORAGE_ROOT_PATH])
        .await
        .expect("roots")[0]
        .iter()
        .map(|cell| cell.value.clone())
        .collect()
}

#[tokio::test]
async fn the_passphrase_is_asked_for_and_a_wrong_one_opens_nothing() {
    let directory = TempDir::new().expect("temp");
    let fixture = windows_archive(directory.path(), &[]).await;
    let wrong = key_for(&fixture.archive, "not the passphrase at all")
        .await
        .expect_err("wrong passphrase");
    assert_eq!(wrong.code, "backup.restore_passphrase_wrong");
    let stray = directory.path().join("stray.rdbackup");
    std::fs::write(&stray, b"just some bytes, no archive").expect("stray");
    let refused = key_for(&stray, PASSPHRASE).await.expect_err("no archive");
    assert_eq!(refused.code, "backup.restore_not_archive");
    // The error names neither passphrase.
    assert!(!format!("{wrong} {refused}").contains(PASSPHRASE));
}

#[tokio::test]
async fn a_preview_reads_every_member_and_changes_nothing() {
    let directory = TempDir::new().expect("temp");
    let fixture = windows_archive(directory.path(), &[]).await;
    // The archive's folder and the top of the test folder: the fixture's own database may
    // still be closing in the background, so its folder is not compared.
    let nas = directory.path().join("nas");
    let top = |path: &Path| {
        let mut names: Vec<_> = std::fs::read_dir(path)
            .expect("folder")
            .flatten()
            .map(|entry| entry.file_name())
            .collect();
        names.sort();
        names
    };
    let before = (listing(&nas), top(directory.path()));

    let key = key_for(&fixture.archive, PASSPHRASE).await.expect("key");
    let contents = read_archive(&fixture.archive, key, |part| {
        part.kind == PartKind::Settings
    })
    .await
    .expect("preview");
    assert_eq!(contents.manifest.app_version, "1.6.0-windows");
    assert!(contents.kept.contains_key(SETTINGS_PART));
    assert!(!contents.kept.contains_key(DATABASE_PART));
    assert!(
        contents
            .manifest
            .parts
            .iter()
            .any(|part| part.name == format!("torrent-session/{INFO_HASH}.torrent"))
    );
    assert_eq!(
        (listing(&nas), top(directory.path())),
        before,
        "the preview wrote something"
    );

    // A damaged archive does not preview: a flipped byte fails its chunk's tag.
    let damaged = directory.path().join("damaged.rdbackup");
    let mut bytes = std::fs::read(&fixture.archive).expect("archive");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x01;
    std::fs::write(&damaged, bytes).expect("damaged");
    let outcome = match key_for(&damaged, PASSPHRASE).await {
        Ok(key) => read_archive(&damaged, key, |_| false).await.map(|_| ()),
        Err(error) => Err(error),
    };
    let error = outcome.expect_err("damaged");
    assert!(
        ["backup.restore_damaged", "backup.restore_passphrase_wrong"].contains(&error.code),
        "{error}"
    );
}

#[tokio::test]
async fn a_test_restore_moves_the_windows_paths_onto_this_system() {
    let directory = TempDir::new().expect("temp");
    let fixture = windows_archive(directory.path(), &[]).await;
    let work = directory.path().join("work");
    prepared(&fixture.archive, &work).await;
    let copy = work.join(DATABASE_PART);
    let schema = restore_copy::copy_schema(&copy).await.expect("schema");
    assert_eq!(schema.pending, 0);
    assert!(!schema.is_newer());

    let downloads = target(directory.path(), "srv-downloads");
    let plan = plan_paths(
        &copy,
        &[RequestedMapping {
            storage_root_id: fixture.root_id.to_string(),
            path: downloads.clone(),
        }],
    )
    .await
    .expect("plan");
    assert_eq!(plan.escapes.count, 0, "{:?}", plan.escapes);
    assert_eq!(plan.moved, 1);
    // The share is below no mapped root: it stays. Elsewhere it is named as foreign; on Windows a
    // UNC path is one this system can reach, so it is not.
    if cfg!(windows) {
        assert_eq!(plan.foreign.count, 0, "{:?}", plan.foreign);
    } else {
        assert_eq!(plan.foreign.count, 1);
        assert_eq!(plan.foreign.examples[0].value, SHARE_PACKAGE);
    }
    assert_eq!(plan.roots.len(), 1);
    assert_eq!(plan.roots[0].path, WINDOWS_ROOT);
    assert!(!plan.roots[0].native || cfg!(windows));

    restore_copy::apply_updates(&copy, &plan.updates)
        .await
        .expect("apply");
    assert_eq!(root_paths(&copy).await, vec![downloads.clone()]);
    let destinations = restore_copy::read_cells(&copy, &PATH_COLUMNS[..1])
        .await
        .expect("destinations");
    let values: Vec<&str> = destinations[0]
        .iter()
        .map(|cell| cell.value.as_str())
        .collect();
    let film = Path::new(&downloads)
        .join("Movies")
        .join("Film (2020)")
        .to_string_lossy()
        .into_owned();
    assert!(values.contains(&film.as_str()), "{values:?}");
    assert!(!values.contains(&WINDOWS_PACKAGE));

    let session = rewrite_session(&work.join("torrent-session"), &plan.mappings, false)
        .await
        .expect("session");
    assert_eq!(session.torrents, 1);
    assert_eq!(session.moved, 1);
    assert!(session.missing_files.is_empty());
    let document: serde_json::Value = serde_json::from_slice(
        &std::fs::read(work.join("torrent-session/session.json")).expect("session"),
    )
    .expect("json");
    assert_eq!(
        document["torrents"]["0"]["output_folder"],
        Path::new(&downloads)
            .join("Movies")
            .to_string_lossy()
            .into_owned()
    );
}

#[tokio::test]
async fn a_stored_parent_step_refuses_the_plan_and_a_bad_mapping_is_named() {
    let directory = TempDir::new().expect("temp");
    let fixture = windows_archive(directory.path(), &[r"D:\Downloads\..\Windows\System32"]).await;
    let work = directory.path().join("work");
    prepared(&fixture.archive, &work).await;
    let copy = work.join(DATABASE_PART);
    let mapping = |path: String| RequestedMapping {
        storage_root_id: fixture.root_id.to_string(),
        path,
    };
    let plan = plan_paths(&copy, &[mapping(target(directory.path(), "srv"))])
        .await
        .expect("plan");
    assert_eq!(plan.escapes.count, 1);
    assert!(plan.escapes.examples[0].value.contains(".."));

    let relative = plan_paths(&copy, &[mapping("relative/folder".to_owned())])
        .await
        .expect_err("relative target");
    assert_eq!(relative.code, "backup.restore_mapping_not_absolute");
    let unknown = plan_paths(
        &copy,
        &[RequestedMapping {
            storage_root_id: "no-such-root".to_owned(),
            path: target(directory.path(), "srv"),
        }],
    )
    .await
    .expect_err("unknown root");
    assert_eq!(unknown.code, "backup.restore_mapping_unknown_root");
    let twice = plan_paths(
        &copy,
        &[
            mapping(target(directory.path(), "a")),
            mapping(target(directory.path(), "b")),
        ],
    )
    .await
    .expect_err("twice");
    assert_eq!(twice.code, "backup.restore_mapping_duplicate");
}

/// A live installation beside the fixture: its database says `/live/root`.
async fn live_installation(directory: &Path, archive: &Path) -> Layout {
    let live = directory.join("live");
    let unpacked = directory.join("live-unpacked");
    prepared(archive, &unpacked).await;
    let copy = unpacked.join(DATABASE_PART);
    let id = restore_copy::read_cells(&copy, &[STORAGE_ROOT_PATH])
        .await
        .expect("roots")[0][0]
        .key
        .clone();
    restore_copy::apply_updates(
        &copy,
        &[CopyUpdate {
            column: STORAGE_ROOT_PATH,
            key: id,
            value: Some("/live/root".to_owned()),
        }],
    )
    .await
    .expect("live root");
    std::fs::create_dir_all(live.join("torrent-session")).expect("live session");
    std::fs::write(
        live.join("torrent-session/session.json"),
        b"{\"torrents\":{}}",
    )
    .expect("live");
    std::fs::rename(&copy, live.join("rdownloader.sqlite3")).expect("live database");
    Layout::new(&live.join("rdownloader.sqlite3"))
}

#[tokio::test]
async fn a_staged_restore_switches_at_the_next_start_and_the_previous_state_goes_after_it() {
    let directory = TempDir::new().expect("temp");
    let fixture = windows_archive(directory.path(), &[]).await;
    let layout = live_installation(directory.path(), &fixture.archive).await;
    let database = layout.data().join("rdownloader.sqlite3");
    let work = directory.path().join("work");
    prepared(&fixture.archive, &work).await;

    cutover::stage(&layout, &work, &pending()).expect("stage");
    assert!(!work.exists());
    assert_eq!(
        cutover::read_marker(&layout)
            .expect("marker")
            .map(|marker| marker.phase),
        Some(Phase::Staged)
    );
    // Staging touched nothing live, and a second restore cannot be staged over it.
    assert_eq!(root_paths(&database).await, vec!["/live/root".to_owned()]);
    let second = directory.path().join("second");
    prepared(&fixture.archive, &second).await;
    assert!(cutover::stage(&layout, &second, &pending()).is_err());

    // The next start.
    let outcome = cutover::apply_pending(&layout).expect("start");
    assert!(matches!(outcome, Cutover::Switched(_)), "{outcome:?}");
    assert_eq!(root_paths(&database).await, vec![WINDOWS_ROOT.to_owned()]);
    let opened = rd_db::Database::open(&database)
        .await
        .expect("open restored");
    assert_eq!(
        opened.list_storage_roots().await.expect("roots")[0].path,
        WINDOWS_ROOT
    );
    assert!(layout.previous().join("rdownloader.sqlite3").exists());
    assert!(
        layout
            .data()
            .join(format!("torrent-session/{INFO_HASH}.torrent"))
            .exists()
    );

    cutover::finish(&layout).expect("finish");
    assert!(cutover::read_marker(&layout).expect("marker").is_none());
    assert!(!layout.previous().exists());
    assert!(!layout.staged().exists());
}

#[tokio::test]
async fn a_restored_database_that_does_not_open_puts_the_previous_one_back() {
    let directory = TempDir::new().expect("temp");
    let fixture = windows_archive(directory.path(), &[]).await;
    let layout = live_installation(directory.path(), &fixture.archive).await;
    let database = layout.data().join("rdownloader.sqlite3");
    let work = directory.path().join("work");
    std::fs::create_dir_all(&work).expect("work");
    std::fs::write(work.join(DATABASE_PART), b"this is no SQLite database").expect("garbage");
    cutover::stage(&layout, &work, &pending()).expect("stage");

    let outcome = cutover::apply_pending(&layout).expect("start");
    assert!(matches!(outcome, Cutover::Switched(_)));
    assert!(rd_db::Database::open(&database).await.is_err());
    let rolled =
        cutover::roll_back(&layout, "the restored database does not open").expect("roll back");
    assert_eq!(
        rolled.map(|marker| marker.minted_secrets),
        Some(vec!["vault://minted".to_owned()])
    );

    // The previous installation is back and starts; the failure is kept with its reason.
    assert_eq!(root_paths(&database).await, vec!["/live/root".to_owned()]);
    rd_db::Database::open(&database)
        .await
        .expect("previous opens");
    let failure = cutover::read_failure(&layout).expect("failure record");
    assert!(failure.reason.contains("does not open"));
    assert!(cutover::read_marker(&layout).expect("marker").is_none());
    assert!(!layout.previous().exists());
    // Dismissing the failure removes it.
    cutover::discard(&layout).expect("discard");
    assert!(cutover::read_failure(&layout).is_none());
}

#[tokio::test]
async fn a_first_start_that_never_completed_is_rolled_back_by_the_next_one() {
    let directory = TempDir::new().expect("temp");
    let fixture = windows_archive(directory.path(), &[]).await;
    let layout = live_installation(directory.path(), &fixture.archive).await;
    let database = layout.data().join("rdownloader.sqlite3");
    let work = directory.path().join("work");
    prepared(&fixture.archive, &work).await;
    cutover::stage(&layout, &work, &pending()).expect("stage");
    assert!(matches!(
        cutover::apply_pending(&layout).expect("first start"),
        Cutover::Switched(_)
    ));
    // The first start stops before it completed; `finish` never ran.
    let outcome = cutover::apply_pending(&layout).expect("second start");
    assert!(matches!(outcome, Cutover::RolledBack { .. }), "{outcome:?}");
    assert_eq!(root_paths(&database).await, vec!["/live/root".to_owned()]);
    let session =
        std::fs::read(layout.data().join("torrent-session/session.json")).expect("session");
    assert_eq!(session, b"{\"torrents\":{}}");
}

#[tokio::test]
async fn a_restore_not_yet_switched_can_be_discarded_and_leaves_the_installation_alone() {
    let directory = TempDir::new().expect("temp");
    let fixture = windows_archive(directory.path(), &[]).await;
    let layout = live_installation(directory.path(), &fixture.archive).await;
    let database = layout.data().join("rdownloader.sqlite3");
    let work = directory.path().join("work");
    prepared(&fixture.archive, &work).await;
    cutover::stage(&layout, &work, &pending()).expect("stage");
    let discarded = cutover::discard(&layout)
        .expect("discard")
        .expect("pending");
    assert_eq!(discarded.minted_secrets, vec!["vault://minted".to_owned()]);
    assert!(!layout.staged().exists());
    assert_eq!(
        cutover::apply_pending(&layout).expect("start"),
        Cutover::Nothing
    );
    assert_eq!(root_paths(&database).await, vec!["/live/root".to_owned()]);
}
