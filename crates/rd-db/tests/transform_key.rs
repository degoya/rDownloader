//! The transform key on its way to the vault, and back (RD-120-11, ADR 0011).
//!
//! RD-110-33 built both ends of this and left the middle out. A stream-transform plugin
//! answers with the key itself and `key_reference: None`; `rd_http::StreamTransform::new`
//! refuses a description that has no reference, on purpose, because the reference is what
//! `ContentTransform::fingerprint` stands the key on. Nothing in between put the key away,
//! so every MEGA download ended as `transform.key_missing` before a byte was fetched.
//!
//! What is proven here is the property that closes it *and* the one acceptance criterion
//! that rests on it: a second attempt at the same file gets the **same** reference, so its
//! fingerprint is unchanged and the chunk MACs of the first attempt are still recognisably
//! its own; a re-resolve that came back with a different key gets a **new** one, so the
//! continuation starts over instead of decrypting with one key over bytes written under
//! another. That is "resume validates node and file identity", at the layer that decides it.
//!
//! And the key itself is searched for in every byte the database wrote, the way
//! `secret_fragment.rs` searches for the fragment.

use rd_db::Database;

/// The file key of the public MEGA example, recomputed in `plugins/mega-common`'s tests from
/// a fragment out of somebody else's README. Nobody's file and nobody's account.
const KEY: [u8; 16] = [
    0x0c, 0x4c, 0x44, 0xe1, 0x28, 0xea, 0xee, 0x7a, 0x40, 0xbc, 0xbd, 0x4f, 0xfe, 0xc1, 0x96, 0x17,
];
/// A different node's key: same row, different file behind it.
const OTHER_KEY: [u8; 16] = [
    0x20, 0x60, 0x07, 0x97, 0x6f, 0x03, 0x52, 0x9a, 0xdd, 0x05, 0xd4, 0xde, 0xc8, 0xb2, 0x0d, 0x1a,
];

async fn open(directory: &std::path::Path, with_vault: bool) -> Database {
    let database = Database::open(directory.join("queue.sqlite3"))
        .await
        .expect("database");
    if with_vault {
        let vault = rd_secrets::SecretStore::open(directory.join("secrets"))
            .await
            .expect("vault");
        database.install_secret_vault(vault);
    }
    database
}

/// One queued file, which is all these tests need a row for.
async fn download(database: &Database, directory: &std::path::Path) -> rd_core::DownloadId {
    let package_id = rd_core::PackageId::new();
    database
        .create_package(rd_db::NewPackage {
            id: package_id,
            name: "mega".to_owned(),
            destination: directory.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    database
        .create_download(rd_db::NewDownload {
            id: rd_core::DownloadId::new(),
            package_id,
            source: "https://mega.nz/file/yuZ0QJ6J".parse().expect("URL"),
            file_name: "10MB.bin".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: rd_core::DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download")
        .id
}

async fn every_stored_byte(database: &Database, directory: &std::path::Path) -> Vec<u8> {
    database.checkpoint_wal().await.expect("checkpoint");
    let mut bytes = Vec::new();
    for entry in std::fs::read_dir(directory).expect("read directory") {
        let path = entry.expect("entry").path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("queue.sqlite3"))
        {
            bytes.extend(std::fs::read(&path).expect("read database file"));
        }
    }
    bytes
}

#[tokio::test]
async fn a_key_is_put_away_once_and_the_same_reference_comes_back() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let id = download(&database, directory.path()).await;

    let first = database
        .adopt_transform_key(id, &KEY)
        .await
        .expect("adopted")
        .expect("a vault is installed");
    assert!(first.starts_with("vault://"));

    // The second attempt at the same file. Same reference, therefore same fingerprint,
    // therefore the chunk MACs the first attempt wrote are still its own.
    let second = database
        .adopt_transform_key(id, &KEY)
        .await
        .expect("adopted")
        .expect("a vault is installed");
    assert_eq!(first, second);

    // And the key really is behind it, byte for byte.
    let vault = database.secret_vault().expect("a vault");
    assert_eq!(vault.get_bytes(&first).await.expect("read"), KEY.to_vec());
}

#[tokio::test]
async fn a_different_key_on_the_same_row_gets_a_new_reference_and_forgets_the_old_one() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let id = download(&database, directory.path()).await;

    let first = database
        .adopt_transform_key(id, &KEY)
        .await
        .expect("adopted")
        .expect("a vault");
    let second = database
        .adopt_transform_key(id, &OTHER_KEY)
        .await
        .expect("adopted")
        .expect("a vault");
    assert_ne!(
        first, second,
        "a different key must change the reference, or the fingerprint would not change \
         and a continuation would decrypt with one key over bytes written under another"
    );
    assert_eq!(
        database
            .download_transform_key_ref(id)
            .await
            .expect("read")
            .as_deref(),
        Some(second.as_str())
    );
    let vault = database.secret_vault().expect("a vault");
    assert!(
        vault.get_bytes(&first).await.is_err(),
        "the superseded key is removed rather than left behind"
    );
    assert_eq!(
        vault.get_bytes(&second).await.expect("read"),
        OTHER_KEY.to_vec()
    );
}

/// Without a vault there is nowhere to put key material, and the answer is "no reference"
/// rather than a description that carries the key instead.
#[tokio::test]
async fn without_a_vault_no_reference_is_invented() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), false).await;
    let id = download(&database, directory.path()).await;
    assert_eq!(
        database.adopt_transform_key(id, &KEY).await.expect("asked"),
        None
    );
    assert_eq!(
        database.download_transform_key_ref(id).await.expect("read"),
        None
    );
}

#[tokio::test]
async fn deleting_the_download_takes_the_key_with_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let id = download(&database, directory.path()).await;
    let reference = database
        .adopt_transform_key(id, &KEY)
        .await
        .expect("adopted")
        .expect("a vault");

    database.delete_download(id).await.expect("delete");
    let vault = database.secret_vault().expect("a vault");
    assert!(vault.get_bytes(&reference).await.is_err());
}

/// The canary. A decryption key in a queue row is the leak the acceptance criterion names.
#[tokio::test]
async fn no_row_the_database_wrote_carries_the_key() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = open(directory.path(), true).await;
    let id = download(&database, directory.path()).await;
    database
        .adopt_transform_key(id, &KEY)
        .await
        .expect("adopted");

    let stored = every_stored_byte(&database, directory.path()).await;
    for spelling in [
        KEY.iter().map(|byte| format!("{byte:02x}")).collect(),
        KEY.iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<String>(),
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, KEY),
    ] {
        assert!(
            !stored
                .windows(spelling.len())
                .any(|window| window == spelling.as_bytes()),
            "the key survived in the database as {spelling:?}"
        );
    }
    assert!(
        !stored
            .windows(KEY.len())
            .any(|window| window == KEY.as_slice()),
        "the key survived in the database as raw bytes"
    );
}
