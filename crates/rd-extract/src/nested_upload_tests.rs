//! RD-170-16: what sits in a folder of the package reaches an upload destination and a plugin
//! step under its path relative to the package, and a confirmed move takes the emptied folder
//! with it.
//!
//! The object storage half — the relative path in the key and a resumed multipart upload of a
//! nested file — is held by `rd-object-storage`'s own tests against its in-memory store.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::PackageId;
use rd_db::{Database, NewPackage};

use crate::{
    ExtractionConfig, ExtractionService, ExtractionTrigger, StorageUpload, StorageUploader,
    UploadReport,
    package_job::package_file_names,
    storage_upload::remove_local,
    tests::{seed_completed_file, zip_bytes},
};

#[tokio::test]
async fn the_package_listing_walks_folders_but_not_links_or_staging() {
    let package = tempfile::tempdir().expect("tempdir");
    let directory = package.path();
    std::fs::create_dir_all(directory.join("Film").join("Extras")).expect("folders");
    std::fs::write(directory.join("top.nfo"), b"n").expect("file");
    std::fs::write(directory.join("Film").join("film.mkv"), b"f").expect("file");
    std::fs::write(directory.join("Film").join("Extras").join("a.srt"), b"s").expect("file");
    // What an interrupted unpack leaves behind is not package content.
    let staging = directory.join(format!("{}abc", rd_postprocess::STAGING_PREFIX));
    std::fs::create_dir_all(&staging).expect("staging");
    std::fs::write(staging.join("half.bin"), b"h").expect("file");
    // Kept alive to the end, so the links point at something real.
    let _outside = tempfile::tempdir().expect("outside");
    #[cfg(unix)]
    {
        std::fs::write(_outside.path().join("secret"), b"x").expect("file");
        std::os::unix::fs::symlink(_outside.path(), directory.join("elsewhere")).expect("link");
        std::os::unix::fs::symlink(
            _outside.path().join("secret"),
            directory.join("Film").join("linked"),
        )
        .expect("link");
    }

    assert_eq!(
        package_file_names(directory).await,
        ["Film/Extras/a.srt", "Film/film.mkv", "top.nfo"]
    );
}

#[tokio::test]
async fn a_confirmed_move_removes_nested_files_and_only_the_folders_it_emptied() {
    let package = tempfile::tempdir().expect("tempdir");
    let directory = package.path();
    std::fs::create_dir_all(directory.join("Film").join("Extras")).expect("folders");
    std::fs::create_dir_all(directory.join("Kept")).expect("folder");
    std::fs::create_dir_all(directory.join("Empty before")).expect("folder");
    std::fs::write(directory.join("Film").join("film.mkv"), b"f").expect("file");
    std::fs::write(directory.join("Film").join("Extras").join("a.srt"), b"s").expect("file");
    std::fs::write(directory.join("Kept").join("done.bin"), b"d").expect("file");
    // Never confirmed, so it stays, and so does its folder.
    std::fs::write(directory.join("Kept").join("not-offered.bin"), b"n").expect("file");

    let removed = remove_local(
        directory,
        &[
            "Film/Extras/a.srt".to_owned(),
            "Film/film.mkv".to_owned(),
            "Kept/done.bin".to_owned(),
        ],
    )
    .await;

    assert_eq!(removed, 3);
    assert!(!directory.join("Film").exists(), "emptied, so removed");
    assert!(directory.join("Kept").join("not-offered.bin").is_file());
    assert!(!directory.join("Kept").join("done.bin").exists());
    assert!(
        directory.join("Empty before").is_dir(),
        "a folder none of these files was in is not touched"
    );
    assert!(directory.is_dir(), "the package directory itself stays");
}

/// A destination that confirms everything and remembers what it was offered.
#[derive(Default)]
struct RecordingDestination {
    offered: Mutex<Vec<String>>,
}

#[async_trait]
impl StorageUploader for RecordingDestination {
    fn installed(&self, _plugin_id: &str) -> bool {
        true
    }

    async fn upload(&self, _plugin_id: &str, upload: StorageUpload<'_>) -> Result<UploadReport> {
        for file in upload.files {
            assert!(
                upload.directory.join(file).is_file(),
                "{file} is offered but cannot be read"
            );
        }
        self.offered
            .lock()
            .expect("offered")
            .extend(upload.files.iter().cloned());
        Ok(UploadReport::Verified {
            files: upload.files.to_vec(),
        })
    }
}

/// Unpacks and moves one `Film.zip` package to the recording destination and returns what it
/// was offered, with the package folder for a look at what is left.
async fn move_through_a_plugin(folders: bool) -> (Vec<String>, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({
                "unpack_to_subfolder": folders,
                "upload_enabled": true,
                "upload_remote": "plugin:recording/https://dav.example/in",
                "upload_mode": "move",
            }),
        )
        .await
        .expect("settings");
    let destination = temp.path().join("dl");
    std::fs::create_dir_all(&destination).expect("destination");
    std::fs::write(destination.join("Film.zip"), zip_bytes("film.mkv", b"film")).expect("zip");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "moved".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    seed_completed_file(&database, package.id, "Film.zip").await;

    let recording = Arc::new(RecordingDestination::default());
    let service = ExtractionService::start_with_plugins(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
        None,
        Some(recording.clone() as Arc<dyn StorageUploader>),
        None,
    );
    service
        .request(package.id, ExtractionTrigger::Manual)
        .await
        .expect("request");
    let state = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = database
                .list_packages()
                .await
                .expect("packages")
                .into_iter()
                .find(|candidate| candidate.id == package.id)
                .expect("package")
                .state;
            if matches!(
                state,
                rd_core::PackageState::Completed | rd_core::PackageState::Failed
            ) {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("post-processing finished");
    service.shutdown().await;
    assert_eq!(state, rd_core::PackageState::Completed);
    let offered = recording.offered.lock().expect("offered").clone();
    (offered, temp)
}

#[tokio::test]
async fn a_plugin_destination_gets_the_relative_path_and_the_move_empties_the_folder() {
    let (offered, temp) = move_through_a_plugin(true).await;
    assert_eq!(offered, ["Film.zip", "Film/film.mkv"]);
    let destination = temp.path().join("dl");
    assert!(
        !destination.join("Film").exists(),
        "moved, so the folder went too"
    );
    assert!(!destination.join("Film.zip").exists());
    assert!(destination.is_dir());
}

#[tokio::test]
async fn a_flat_package_is_offered_as_before() {
    let (offered, temp) = move_through_a_plugin(false).await;
    assert_eq!(offered, ["Film.zip", "film.mkv"]);
    assert!(!temp.path().join("dl").join("film.mkv").exists());
}
