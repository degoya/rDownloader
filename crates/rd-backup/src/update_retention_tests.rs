use std::path::Path;
use std::time::{Duration, SystemTime};

use chrono::{TimeZone, Utc};

use super::*;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Writes `name` into `folder` with `bytes` bytes, last written `age` ago.
fn file(folder: &Path, name: &str, bytes: usize, age: Duration) -> PathBuf {
    std::fs::create_dir_all(folder).expect("folder");
    let path = folder.join(name);
    std::fs::write(&path, vec![0_u8; bytes]).expect("write");
    let written = SystemTime::now() - age;
    std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("open")
        .set_modified(written)
        .expect("mtime");
    path
}

fn copy_name(from: &str, to: &str, day: u32) -> String {
    rd_db::pre_migration::copy_name(
        from,
        to,
        Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0)
            .single()
            .expect("time"),
    )
}

fn archive_name(target: &str, day: u32) -> String {
    crate::archive_name(
        &format!("pre-update-{target}"),
        Utc.with_ymd_and_hms(2026, 10, day, 12, 0, 0)
            .single()
            .expect("time"),
    )
}

/// Three updates' worth of copies and archives, three migrations' copies, and what is not
/// theirs: a `.partial` copy, a stranger and the staging folder.
struct Data {
    _root: tempfile::TempDir,
    data: PathBuf,
    newest_copy: PathBuf,
    newest_archive: PathBuf,
    newest_migration: PathBuf,
    untouchable: Vec<PathBuf>,
}

fn data(newest_age: Duration) -> Data {
    let root = tempfile::tempdir().expect("tempdir");
    let data = root.path().join("data");
    let pre_update = data.join(pre_update::DIRECTORY);
    let pre_migration = data.join(rd_db::pre_migration::DIRECTORY);
    let old = 40 * DAY;
    file(&pre_update, &copy_name("1.22.0", "1.23.0", 1), 100, old);
    file(&pre_update, &copy_name("1.23.0", "1.23.1", 2), 100, old);
    let newest_copy = file(
        &pre_update,
        &copy_name("1.23.1", "1.24.0", 3),
        100,
        newest_age,
    );
    file(&pre_update, &archive_name("1.23.0", 1), 50, old);
    file(&pre_update, &archive_name("1.23.1", 2), 50, old);
    let newest_archive = file(&pre_update, &archive_name("1.24.0", 3), 50, newest_age);
    file(&pre_migration, &copy_name("0137", "0138", 1), 10, old);
    file(&pre_migration, &copy_name("0138", "0139", 2), 10, old);
    let newest_migration = file(
        &pre_migration,
        &copy_name("0139", "0141", 3),
        10,
        newest_age,
    );
    let untouchable = vec![
        file(
            &pre_update,
            &format!("{}.partial", copy_name("1.24.0", "1.25.0", 4)),
            100,
            old,
        ),
        file(&pre_update, "notes.txt", 5, old),
        file(&pre_update.join("staging"), "pre-update", 5, old),
        file(&pre_migration, "rdownloader.sqlite3", 5, old),
    ];
    Data {
        _root: root,
        data,
        newest_copy,
        newest_archive,
        newest_migration,
        untouchable,
    }
}

fn policy(proven: bool, grace_days: u32) -> UpdateBackupPolicy {
    UpdateBackupPolicy {
        proven,
        grace_days,
        now: SystemTime::now(),
    }
}

fn remaining(folder: &Path) -> usize {
    std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .count()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_proven_update_keeps_exactly_the_newest_of_each_kind() {
    let fixture = data(DAY);
    let removed = apply(&fixture.data, policy(true, 14)).await;
    assert_eq!(removed.removable.len(), 6, "{removed:?}");
    assert_eq!(removed.removable_of(&[BackupKind::PreUpdateCopy]), (2, 200));
    assert_eq!(
        removed.removable_of(&[BackupKind::PreUpdateArchive]),
        (2, 100)
    );
    assert_eq!(
        removed.removable_of(&[BackupKind::PreMigrationCopy]),
        (2, 20)
    );
    let kept: Vec<_> = removed.kept.iter().map(|file| file.path.clone()).collect();
    assert_eq!(kept.len(), 3);
    for newest in [
        &fixture.newest_copy,
        &fixture.newest_archive,
        &fixture.newest_migration,
    ] {
        assert!(newest.exists(), "{}", newest.display());
        assert!(kept.contains(newest));
    }
    for other in &fixture.untouchable {
        assert!(other.exists(), "{} is not ours", other.display());
    }
    // The newest copy, the newest archive, the `.partial` copy and the stranger.
    assert_eq!(remaining(&fixture.data.join(pre_update::DIRECTORY)), 4);
}

#[tokio::test]
async fn an_unproven_update_keeps_everything() {
    let fixture = data(40 * DAY);
    let plan = apply(&fixture.data, policy(false, 14)).await;
    assert!(plan.removable.is_empty(), "{plan:?}");
    assert_eq!(plan.kept.len(), 9);
    assert_eq!(remaining(&fixture.data.join(pre_update::DIRECTORY)), 8);
    assert_eq!(
        remaining(&fixture.data.join(rd_db::pre_migration::DIRECTORY)),
        4
    );
}

#[tokio::test]
async fn the_newest_goes_once_its_grace_has_run_out_unless_kept_for_good() {
    let fixture = data(15 * DAY);
    let preview = plan(&fixture.data, policy(true, 14)).await;
    assert_eq!(preview.removable.len(), 9, "a preview removes nothing");
    assert!(fixture.newest_copy.exists());

    let forever = plan(&fixture.data, policy(true, 0)).await;
    assert_eq!(forever.kept.len(), 3, "0 keeps the newest for good");

    let young = data(13 * DAY);
    assert_eq!(plan(&young.data, policy(true, 14)).await.kept.len(), 3);

    apply(&fixture.data, policy(true, 14)).await;
    assert!(!fixture.newest_copy.exists());
    assert!(!fixture.newest_archive.exists());
    assert!(!fixture.newest_migration.exists());
    for other in &fixture.untouchable {
        assert!(other.exists(), "{} is not ours", other.display());
    }
}

#[tokio::test]
async fn missing_folders_are_an_empty_plan() {
    let root = tempfile::tempdir().expect("tempdir");
    let plan = apply(root.path(), policy(true, 14)).await;
    assert!(plan.kept.is_empty() && plan.removable.is_empty());
}
