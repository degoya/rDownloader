//! Verifying an archive where it lies (RD-160-02): an untouched archive passes, a changed
//! byte, a cut and a missing file each fail with their own code, and an archive from before the
//! passphrase was replaced is checked by its digest alone.

use rd_backup::{
    BackupDestination, BackupKey, BackupSources, LocalFolder,
    archive::verify_archive,
    seal_backup, staging_root,
    verify::{ExpectedArchive, VERIFY_DAMAGED, VERIFY_DIGEST_MISMATCH, VERIFY_MISSING, verify_at},
};
use rd_db::Database;

struct Written {
    directory: tempfile::TempDir,
    folder: LocalFolder,
    key: BackupKey,
    expected: ExpectedArchive,
}

async fn written() -> Written {
    let directory = tempfile::tempdir().expect("temp");
    let data = directory.path().join("data");
    let database = Database::open(data.join("rdownloader.sqlite3"))
        .await
        .expect("database");
    let key = BackupKey::derive_new("correct horse battery")
        .await
        .expect("key");
    let sealed = seal_backup(
        &database,
        BackupSources {
            settings_bundle: b"{}".to_vec(),
            torrent_session: None,
            torrent_files: None,
            app_version: "test".to_owned(),
            instance_id: "0a1b2c3d".to_owned(),
        },
        &key,
        &staging_root(&data),
        "run",
        chrono::Utc::now(),
    )
    .await
    .expect("seal");
    let folder = LocalFolder::open(&directory.path().join("nas"))
        .await
        .expect("folder");
    folder
        .store(&sealed.path, &sealed.archive_name)
        .await
        .expect("store");
    Written {
        expected: ExpectedArchive {
            name: sealed.archive_name,
            size_bytes: sealed.size_bytes,
            sha256: sealed.sha256,
        },
        directory,
        folder,
        key,
    }
}

impl Written {
    fn stored(&self) -> std::path::PathBuf {
        self.folder.path().join(&self.expected.name)
    }

    fn scratch(&self) -> std::path::PathBuf {
        self.directory.path().join("scratch")
    }
}

#[tokio::test]
async fn an_untouched_archive_passes_with_its_content_checked() {
    let written = written().await;
    let verified = verify_at(
        &written.folder,
        &written.expected,
        &written.key,
        &written.scratch(),
    )
    .await
    .expect("verified");
    assert!(verified.content_checked);
    // The copy it checked is gone again.
    assert_eq!(
        std::fs::read_dir(written.scratch())
            .expect("scratch")
            .count(),
        0
    );
}

#[tokio::test]
async fn a_changed_byte_and_a_cut_are_found() {
    let written = written().await;
    let original = std::fs::read(written.stored()).expect("archive");

    let mut changed = original.clone();
    let middle = changed.len() / 2;
    changed[middle] ^= 0x01;
    std::fs::write(written.stored(), &changed).expect("tamper");
    let tampered = verify_at(
        &written.folder,
        &written.expected,
        &written.key,
        &written.scratch(),
    )
    .await
    .expect_err("tampered");
    assert_eq!(tampered.code, VERIFY_DIGEST_MISMATCH);

    std::fs::write(written.stored(), &original[..original.len() - 100]).expect("cut");
    let cut = verify_at(
        &written.folder,
        &written.expected,
        &written.key,
        &written.scratch(),
    )
    .await
    .expect_err("cut");
    assert_eq!(cut.code, VERIFY_DIGEST_MISMATCH);

    // Without the ledger's digest the content check alone finds both as well.
    let copy = written.directory.path().join("copy.rdbackup");
    std::fs::write(&copy, &changed).expect("copy");
    assert!(verify_archive(&copy, &written.key).is_err());
    std::fs::write(&copy, &original[..original.len() - 100]).expect("copy");
    assert!(verify_archive(&copy, &written.key).is_err());
    std::fs::write(&copy, &original).expect("copy");
    verify_archive(&copy, &written.key).expect("the original passes");

    // A ledger that describes the damaged file (as if it had been damaged before the upload)
    // still does not let it pass: the content check fails it.
    std::fs::write(written.stored(), &changed).expect("tamper");
    let (size_bytes, sha256) = rd_backup::archive::digest_file(&written.stored()).expect("digest");
    let damaged = ExpectedArchive {
        name: written.expected.name.clone(),
        size_bytes,
        sha256,
    };
    let refused = verify_at(&written.folder, &damaged, &written.key, &written.scratch())
        .await
        .expect_err("damaged");
    assert_eq!(refused.code, VERIFY_DAMAGED);
}

#[tokio::test]
async fn a_missing_archive_is_named_missing() {
    let written = written().await;
    std::fs::remove_file(written.stored()).expect("remove");
    let missing = verify_at(
        &written.folder,
        &written.expected,
        &written.key,
        &written.scratch(),
    )
    .await
    .expect_err("missing");
    assert_eq!(missing.code, VERIFY_MISSING);
}

#[tokio::test]
async fn an_archive_under_an_earlier_passphrase_is_checked_by_its_digest() {
    let written = written().await;
    let replaced = BackupKey::derive_new("another long passphrase")
        .await
        .expect("key");
    let verified = verify_at(
        &written.folder,
        &written.expected,
        &replaced,
        &written.scratch(),
    )
    .await
    .expect("digest passes");
    assert!(!verified.content_checked);
}

/// RD-1190-22: the scratch folder holds a copy of the archive while it is checked, and in a data
/// directory that already exists open -- a Docker volume -- it was left open too.
#[cfg(unix)]
#[tokio::test]
async fn the_scratch_folder_is_the_service_accounts_alone() {
    use std::os::unix::fs::PermissionsExt;

    let written = written().await;
    std::fs::create_dir_all(written.scratch()).expect("scratch");
    std::fs::set_permissions(written.scratch(), std::fs::Permissions::from_mode(0o755))
        .expect("open it");
    verify_at(
        &written.folder,
        &written.expected,
        &written.key,
        &written.scratch(),
    )
    .await
    .expect("verified");
    let mode = std::fs::metadata(written.scratch())
        .expect("scratch")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700, "{mode:o}");
}
