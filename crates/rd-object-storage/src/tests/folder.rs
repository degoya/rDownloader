//! One folder of a bucket as a plain file store (RD-160-02), against memory: put, list, get and
//! delete, the multipart upload that continues after a failed part, and the folder as the only
//! thing a name can reach.

use object_store::{GetOptions, ObjectStore, path::Path};
use tokio_util::sync::CancellationToken;

use super::{Harness, MIB, lock, payload};
use crate::{FOLDER_NAME_INVALID, FOLDER_PROFILE_MISSING, ObjectFolder};

async fn folder(harness: &Harness) -> ObjectFolder {
    let profile = harness.profile().await;
    harness
        .service
        .open_folder(&profile.id.to_string(), "media-bucket/backups")
        .await
        .expect("database")
        .expect("folder")
}

async fn local_file(harness: &Harness, name: &str, length: usize) -> std::path::PathBuf {
    let path = harness.directory.path().join(name);
    tokio::fs::write(&path, payload(length))
        .await
        .expect("file");
    path
}

async fn put(folder: &ObjectFolder, name: &str, local: &std::path::Path) -> Result<u64, String> {
    folder
        .put_file(
            name,
            local,
            &format!("backup:{name}"),
            &rd_limits::ScopedLimiter::unlimited(),
            &CancellationToken::new(),
        )
        .await
        .expect("database")
        .map_err(|failure| failure.message)
}

#[tokio::test]
async fn a_file_goes_up_is_listed_comes_back_and_is_deleted() {
    let harness = Harness::start().await;
    let folder = folder(&harness).await;
    assert_eq!(folder.describe(), "media-bucket/backups");
    let local = local_file(&harness, "archive.rdbackup", 4096).await;
    assert_eq!(put(&folder, "a.rdbackup", &local).await, Ok(4096));
    // The local file stays where it was.
    assert!(local.exists());
    assert_eq!(
        folder.size_of("a.rdbackup").await.expect("head"),
        Some(4096)
    );
    let listed = folder.list().await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "a.rdbackup");
    assert_eq!(listed[0].size, 4096);

    let back = harness.directory.path().join("back.rdbackup");
    let written = folder
        .get_file("a.rdbackup", &back)
        .await
        .expect("disk")
        .expect("get");
    assert_eq!(written, 4096);
    assert_eq!(std::fs::read(&back).expect("read"), payload(4096));

    assert!(folder.delete("a.rdbackup").await.expect("delete"));
    assert!(!folder.delete("a.rdbackup").await.expect("delete again"));
    assert!(folder.list().await.expect("list").is_empty());
    let missing = folder
        .get_file("a.rdbackup", &harness.directory.path().join("missing"))
        .await
        .expect("disk")
        .expect_err("gone");
    assert_eq!(missing.code.as_deref(), Some(crate::error::NOT_FOUND));
}

#[tokio::test]
async fn a_large_file_continues_at_its_first_missing_part() {
    let harness = Harness::start().await;
    let folder = folder(&harness).await;
    let local = local_file(&harness, "big.rdbackup", 40 * MIB).await;
    *lock(&harness.parts.fail_part) = Some(1);
    assert!(put(&folder, "big.rdbackup", &local).await.is_err());
    // Nothing is visible under the name while the upload is unfinished.
    assert_eq!(folder.size_of("big.rdbackup").await.expect("head"), None);

    assert_eq!(
        put(&folder, "big.rdbackup", &local).await,
        Ok(40 * MIB as u64)
    );
    // Part 0 went up once.
    assert_eq!(*lock(&harness.parts.sent), vec![0, 1, 2]);
    let stored = harness
        .memory
        .get_opts(&Path::from("backups/big.rdbackup"), GetOptions::default())
        .await
        .expect("object")
        .bytes()
        .await
        .expect("bytes");
    assert_eq!(stored.len(), 40 * MIB);
    assert!(
        harness
            .database
            .object_uploads(None, None)
            .await
            .expect("records")
            .is_empty()
    );
}

#[tokio::test]
async fn a_name_reaches_only_the_folder() {
    let harness = Harness::start().await;
    let folder = folder(&harness).await;
    harness
        .put("backups/deeper/other.rdbackup", payload(10))
        .await;
    harness.put("elsewhere.rdbackup", payload(10)).await;
    // Neither the deeper key nor the one beside the folder is part of it.
    assert!(folder.list().await.expect("list").is_empty());
    for name in ["../elsewhere.rdbackup", "deeper/other.rdbackup", "", ".."] {
        let refused = folder.delete(name).await.expect_err(name);
        assert_eq!(refused.code.as_deref(), Some(FOLDER_NAME_INVALID), "{name}");
    }
    let unknown = harness
        .service
        .open_folder("not-a-profile", "media-bucket")
        .await
        .expect("database")
        .expect_err("no profile");
    assert_eq!(unknown.code.as_deref(), Some(FOLDER_PROFILE_MISSING));
}
