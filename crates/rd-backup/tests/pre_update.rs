//! The backup before an update (RD-180-03): the database copy is checked before it carries its
//! name and matches the schema the running version needs, the archive opens again under its
//! key, each keeps its own newest three, and the sweep removes only what a stop left.

use std::path::Path;

use chrono::{Duration, Utc};
use rd_backup::{
    BackupKey, BackupSources,
    archive::verify_archive,
    pre_update::{self, UpdatePlan},
};
use rd_core::BackupOrigin;
use rd_db::{Database, NewBackupRun};
use tempfile::TempDir;

fn sources() -> BackupSources {
    BackupSources {
        settings_bundle: b"{}".to_vec(),
        torrent_session: None,
        torrent_files: None,
        app_version: "1.8.0-beta.1".to_owned(),
        instance_id: "0a1b2c3d".to_owned(),
    }
}

fn plan(data: &Path, at: chrono::DateTime<Utc>) -> UpdatePlan<'_> {
    UpdatePlan {
        data_directory: data,
        from_version: "1.8.0-beta.1",
        target_version: "1.8.0-beta.2",
        at,
    }
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

async fn open(directory: &TempDir) -> (Database, std::path::PathBuf) {
    let data = directory.path().join("data");
    let database = Database::open(data.join("rdownloader.sqlite3"))
        .await
        .expect("database");
    assert!(
        database
            .begin_backup_run(NewBackupRun {
                id: "before-the-update".to_owned(),
                origin: BackupOrigin::Manual,
                started_at: Utc::now(),
                destination_id: None,
                destination: None,
            })
            .await
            .expect("begin")
    );
    (database, data)
}

#[tokio::test]
async fn the_copy_is_checked_named_by_both_versions_and_holds_the_live_rows() {
    let directory = TempDir::new().expect("temp");
    let (database, data) = open(&directory).await;
    let copy = pre_update::copy_database(&database, plan(&data, Utc::now()))
        .await
        .expect("copy");
    let name = copy
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .expect("name");
    assert!(
        name.starts_with("rdownloader-1.8.0-beta.1-to-1.8.0-beta.2-") && name.ends_with(".sqlite3"),
        "{name}"
    );
    assert_eq!(
        copy.path.parent(),
        Some(pre_update::directory(&data).as_path())
    );
    assert!(copy.size_bytes > 0);
    // The schema of this build, nothing pending: the copy fits the version that wrote it.
    let latest = rd_db::restore_copy::copy_schema(&data.join("rdownloader.sqlite3"))
        .await
        .expect("live schema");
    assert_eq!(Some(copy.schema_version), latest.applied);
    rd_db::snapshot::check_integrity(&copy.path)
        .await
        .expect("whole");
    let runs = rd_db::snapshot::read_tables(&copy.path, &["backup_runs"])
        .await
        .expect("read copy");
    assert_eq!(runs["backup_runs"].len(), 1);
    // Nothing under a working name is left beside it.
    assert_eq!(names(&pre_update::directory(&data)), vec![name.to_owned()]);
}

#[tokio::test]
async fn copies_and_archives_each_keep_their_newest_three() {
    let directory = TempDir::new().expect("temp");
    let (database, data) = open(&directory).await;
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    let start = Utc::now();
    for step in 0..5 {
        let at = start + Duration::seconds(step);
        pre_update::copy_database(&database, plan(&data, at))
            .await
            .expect("copy");
        pre_update::seal_archive(&database, sources(), &key, plan(&data, at))
            .await
            .expect("archive");
    }
    let names = names(&pre_update::directory(&data));
    let copies = names
        .iter()
        .filter(|name| name.ends_with(".sqlite3"))
        .count();
    let archives: Vec<&String> = names
        .iter()
        .filter(|name| name.ends_with(".rdbackup"))
        .collect();
    assert_eq!(copies, pre_update::KEPT, "{names:?}");
    assert_eq!(archives.len(), pre_update::KEPT, "{names:?}");
    // The newest stayed: the last one written is among them.
    let newest = (start + Duration::seconds(4))
        .format("%Y%m%dT%H%M%SZ")
        .to_string();
    assert!(
        archives.iter().any(|name| name.contains(&newest)),
        "{names:?}"
    );
    // No staging and no unencrypted copy of the archive's database is left behind.
    assert!(!pre_update::directory(&data).join("staging").exists());
}

/// Finding 8 of the 2026-09-30 review: the plain database copies lay in a folder the umask left
/// readable by every account on the machine.
#[cfg(unix)]
#[tokio::test]
async fn the_folder_of_the_plain_copies_is_the_service_accounts_alone() {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = |path: &Path| {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    };
    let directory = TempDir::new().expect("temp");
    let (database, data) = open(&directory).await;
    let folder = pre_update::directory(&data);
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    pre_update::copy_database(&database, plan(&data, Utc::now()))
        .await
        .expect("copy");
    assert_eq!(mode(&folder), 0o700);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    pre_update::seal_archive(&database, sources(), &key, plan(&data, Utc::now()))
        .await
        .expect("archive");
    assert_eq!(mode(&folder), 0o700);
}

#[tokio::test]
async fn the_archive_opens_again_under_its_key_and_only_under_it() {
    let directory = TempDir::new().expect("temp");
    let (database, data) = open(&directory).await;
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    let archive = pre_update::seal_archive(&database, sources(), &key, plan(&data, Utc::now()))
        .await
        .expect("archive");
    assert_eq!(archive.key_fingerprint, key.fingerprint());
    let manifest = verify_archive(&archive.path, &key).expect("verifies");
    assert!(!manifest.parts.is_empty());
    let other = BackupKey::derive_new("another long passphrase")
        .await
        .expect("key");
    assert!(verify_archive(&archive.path, &other).is_err());
    let (size, sha256) = rd_backup::archive::digest_file(&archive.path).expect("digest");
    assert_eq!((size, sha256), (archive.size_bytes, archive.sha256));
}

#[tokio::test]
async fn the_sweep_removes_leftovers_and_never_a_published_copy() {
    let directory = TempDir::new().expect("temp");
    let (database, data) = open(&directory).await;
    let copy = pre_update::copy_database(&database, plan(&data, Utc::now()))
        .await
        .expect("copy");
    let folder = pre_update::directory(&data);
    std::fs::write(
        folder.join("rdownloader-x-to-y-stamp.sqlite3.partial"),
        b"half",
    )
    .expect("partial");
    std::fs::create_dir_all(folder.join("staging/pre-update")).expect("staging");
    std::fs::write(folder.join("staging/pre-update/database.sqlite3"), b"plain").expect("staged");
    pre_update::sweep(&data).await.expect("sweep");
    assert!(copy.path.exists());
    assert!(!folder.join("staging").exists());
    assert_eq!(names(&folder).len(), 1);
    // A data directory that never prepared an update has nothing to sweep.
    let empty = TempDir::new().expect("temp");
    pre_update::sweep(empty.path())
        .await
        .expect("nothing to sweep");
}

#[tokio::test]
async fn a_version_that_is_no_file_name_writes_nothing() {
    let directory = TempDir::new().expect("temp");
    let (database, data) = open(&directory).await;
    let mut bad = plan(&data, Utc::now());
    bad.target_version = "../../escape";
    let error = pre_update::copy_database(&database, bad)
        .await
        .expect_err("refused");
    assert_eq!(error.code, pre_update::COPY_FAILED);
    assert!(!pre_update::directory(&data).exists());
}
