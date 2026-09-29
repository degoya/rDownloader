//! The fixture the restore tests share (RD-160-03): an archive made by an installation whose
//! paths are Windows paths — a storage root on `D:\Downloads`, a package below it, one on a
//! share nothing maps, and a torrent whose output folder lies below the root.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use rd_backup::{BackupKey, BackupSources, LocalFolder, create_backup, staging_root};
use rd_core::{PackageId, StorageRootId};
use rd_db::{Database, NewPackage, NewStorageRoot};

pub const PASSPHRASE: &str = "correct horse battery staple";
pub const WINDOWS_ROOT: &str = r"D:\Downloads";
pub const WINDOWS_PACKAGE: &str = r"D:\Downloads\Movies\Film (2020)";
pub const SHARE_PACKAGE: &str = r"\\nas\media\Series";
pub const TORRENT_OUTPUT: &str = r"D:\Downloads\Movies";
pub const INFO_HASH: &str = "0123456789abcdef0123456789abcdef01234567";

pub struct Fixture {
    pub archive: PathBuf,
    pub root_id: StorageRootId,
}

async fn package(database: &Database, name: &str, destination: &str) {
    database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: name.to_owned(),
            destination: destination.to_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
}

/// Writes the archive into `directory/nas`; `extra` adds packages with the given destinations.
pub async fn windows_archive(directory: &Path, extra: &[&str]) -> Fixture {
    let data = directory.join("windows-data");
    let database = Database::open(data.join("rdownloader.sqlite3"))
        .await
        .expect("database");
    let root_id = StorageRootId::new();
    database
        .create_storage_root(
            root_id,
            NewStorageRoot {
                name: "Main".to_owned(),
                path: WINDOWS_ROOT.to_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    package(&database, "Film", WINDOWS_PACKAGE).await;
    package(&database, "Series", SHARE_PACKAGE).await;
    for (index, destination) in extra.iter().enumerate() {
        package(&database, &format!("Extra {index}"), destination).await;
    }
    let session = data.join("torrent-session");
    std::fs::create_dir_all(&session).expect("session");
    std::fs::write(
        session.join("session.json"),
        serde_json::to_vec(&serde_json::json!({
            "torrents": {
                "0": {
                    "info_hash": INFO_HASH,
                    "trackers": [],
                    "output_folder": TORRENT_OUTPUT,
                    "only_files": null,
                    "is_paused": true,
                }
            }
        }))
        .expect("json"),
    )
    .expect("session file");
    std::fs::write(session.join(format!("{INFO_HASH}.torrent")), b"d4:infoe").expect("torrent");

    let key = BackupKey::derive_new(PASSPHRASE).await.expect("key");
    let nas = LocalFolder::open(&directory.join("nas"))
        .await
        .expect("destination");
    let created = create_backup(
        &database,
        BackupSources {
            settings_bundle: br#"{"format":"rdownloader-settings-bundle"}"#.to_vec(),
            torrent_session: Some(session),
            torrent_files: None,
            app_version: "1.6.0-windows".to_owned(),
            instance_id: "0a1b2c3d".to_owned(),
        },
        &key,
        &nas,
        &staging_root(&data),
        "fixture",
        chrono::Utc::now(),
    )
    .await
    .expect("backup");
    Fixture {
        archive: PathBuf::from(created.location),
        root_id,
    }
}

/// A folder name this system reads as absolute, below `base`.
pub fn target(base: &Path, name: &str) -> String {
    base.join(name).to_string_lossy().into_owned()
}

/// Every file below `root` with its size, sorted: what "changed nothing" is compared by.
pub fn listing(root: &Path) -> Vec<(PathBuf, u64)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.metadata() {
                Ok(metadata) if metadata.is_dir() => pending.push(path),
                Ok(metadata) => found.push((path, metadata.len())),
                Err(_) => {}
            }
        }
    }
    found.sort();
    found
}
