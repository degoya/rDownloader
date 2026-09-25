use std::{io::Write, time::Duration};

use rd_core::{DownloadId, DownloadState, PackageId, PostprocessKind, PostprocessState};
use rd_db::{Database, NewDownload, NewPackage, PackageChange};
use zip::write::SimpleFileOptions;

use crate::{ExtractionConfig, ExtractionService, ExtractionTrigger};

fn write_zip(path: &std::path::Path, options: SimpleFileOptions, name: &str) {
    let file = std::fs::File::create(path).expect("create ZIP");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file(name, options).expect("start member");
    zip.write_all(b"payload").expect("write member");
    zip.finish().expect("finish ZIP");
}

fn zip_bytes(member_name: &str, content: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file(member_name, SimpleFileOptions::default())
        .expect("start member");
    zip.write_all(content).expect("write member");
    zip.finish().expect("finish ZIP").into_inner()
}

/// `level1.zip ⊃ level2.zip ⊃ … ⊃ payload.txt`: extracting `levels` times reaches the payload.
fn nested_zip(levels: usize) -> Vec<u8> {
    let mut name = "payload.txt".to_owned();
    let mut bytes = b"payload".to_vec();
    for level in (1..=levels).rev() {
        bytes = zip_bytes(&name, &bytes);
        name = format!("level{level}.zip");
    }
    bytes
}

/// Creates a completed one-file package holding `level1.zip` with the given nesting depth.
async fn seed_nested_package(
    database: &Database,
    destination: &std::path::Path,
    levels: usize,
) -> PackageId {
    std::fs::create_dir_all(destination).expect("destination");
    std::fs::write(destination.join("level1.zip"), nested_zip(levels)).expect("archive");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "nested".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let file = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id: package.id,
            source: "https://example.test/level1.zip".parse().expect("URL"),
            file_name: "level1.zip".to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    for state in [
        DownloadState::Resolving,
        DownloadState::Downloading,
        DownloadState::Verifying,
        DownloadState::Completed,
    ] {
        database
            .transition_download(file.id, state)
            .await
            .expect("transition");
    }
    package.id
}

async fn run_extraction(database: &Database, temp: &std::path::Path, package_id: PackageId) {
    let state = run_extraction_to_end(database, temp, package_id).await;
    assert_eq!(state, rd_core::PackageState::Completed);
}

/// Runs the pipeline and returns whichever terminal state the package reached.
async fn run_extraction_to_end(
    database: &Database,
    temp: &std::path::Path,
    package_id: PackageId,
) -> rd_core::PackageState {
    run_extraction_with(database, temp, package_id, ExtractionTrigger::Manual).await
}

/// The same, with the trigger spelled out — `Force` is the "post-process anyway" action.
async fn run_extraction_with(
    database: &Database,
    temp: &std::path::Path,
    package_id: PackageId,
    trigger: ExtractionTrigger,
) -> rd_core::PackageState {
    let service = ExtractionService::start(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
        },
    );
    service.request(package_id, trigger).await.expect("request");
    let state = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let package = database
                .list_packages()
                .await
                .expect("packages")
                .into_iter()
                .find(|package| package.id == package_id)
                .expect("package");
            if matches!(
                package.state,
                rd_core::PackageState::Completed | rd_core::PackageState::Failed
            ) {
                break package.state;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("extraction finished");
    service.shutdown().await;
    state
}

#[tokio::test]
async fn recursive_unpack_extracts_nested_archives_and_deletes_intermediates() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "recursive_unpack": true }),
        )
        .await
        .expect("settings");
    let destination = temp.path().join("dl");
    let package_id = seed_nested_package(&database, &destination, 2).await;
    run_extraction(&database, temp.path(), package_id).await;
    assert_eq!(
        std::fs::read(destination.join("payload.txt")).ok(),
        Some(b"payload".to_vec())
    );
    // The inner archive is an intermediate and gets deleted; the original volume follows
    // the package's level (Unpack, no delete) and stays.
    assert!(!destination.join("level2.zip").exists());
    assert!(destination.join("level1.zip").exists());
}

#[tokio::test]
async fn recursive_unpack_stops_at_the_depth_cap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "recursive_unpack": true }),
        )
        .await
        .expect("settings");
    let destination = temp.path().join("dl");
    // Five levels, but only the initial pass plus 3 recursive passes run.
    let package_id = seed_nested_package(&database, &destination, 5).await;
    run_extraction(&database, temp.path(), package_id).await;
    assert!(destination.join("level5.zip").exists());
    assert!(!destination.join("level4.zip").exists());
    assert!(!destination.join("payload.txt").exists());
}

#[tokio::test]
async fn nested_archives_stay_untouched_without_recursive_unpack() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("dl");
    let package_id = seed_nested_package(&database, &destination, 2).await;
    run_extraction(&database, temp.path(), package_id).await;
    assert!(destination.join("level2.zip").exists());
    assert!(!destination.join("payload.txt").exists());
}

#[tokio::test]
async fn manual_extraction_uses_package_password_list_and_deletes_originals() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("dl");
    std::fs::create_dir_all(&destination).expect("destination");
    write_zip(
        &destination.join("plain.zip"),
        SimpleFileOptions::default(),
        "plain.txt",
    );
    write_zip(
        &destination.join("locked.zip"),
        SimpleFileOptions::default().with_aes_encryption(zip::AesMode::Aes256, "listed-pw"),
        "locked.txt",
    );
    std::fs::write(temp.path().join("passwords.txt"), "wrong\nlisted-pw\n").expect("passwords");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "delete_archives_after_extract": true }),
        )
        .await
        .expect("settings");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "archives".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    for name in ["plain.zip", "locked.zip"] {
        let file = database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id: package.id,
                source: format!("https://example.test/{name}").parse().expect("URL"),
                file_name: name.to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                initial_state: DownloadState::Queued,
                kind: rd_core::DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                mirror_group: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download");
        for state in [
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
            DownloadState::Completed,
        ] {
            database
                .transition_download(file.id, state)
                .await
                .expect("transition");
        }
    }
    database
        .update_packages(
            vec![package.id],
            PackageChange {
                category: None,
                priority: None,
                name: None,
                password: Some(Some("unused-package-pw".to_owned())),
                postprocess_level: None,
                script: None,
            },
        )
        .await
        .expect("password");
    let stored = database.list_packages().await.expect("packages");
    assert!(stored[0].has_password);
    // RD-104-04: an archive password is readable, not just countable.
    assert_eq!(stored[0].password.as_deref(), Some("unused-package-pw"));

    let service = ExtractionService::start(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
        },
    );
    service
        .request(package.id, ExtractionTrigger::Manual)
        .await
        .expect("request");
    let owner = package.id.to_string();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let steps = database
                .list_postprocess_steps(&owner)
                .await
                .expect("steps");
            let done = steps
                .iter()
                .filter(|step| {
                    step.kind == PostprocessKind::DeleteArchives
                        && step.state == PostprocessState::Completed
                })
                .count();
            if done == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("extraction finished");
    assert_eq!(
        std::fs::read(destination.join("plain.txt")).ok(),
        Some(b"payload".to_vec())
    );
    assert_eq!(
        std::fs::read(destination.join("locked.txt")).ok(),
        Some(b"payload".to_vec())
    );
    assert!(!destination.join("plain.zip").exists());
    assert!(!destination.join("locked.zip").exists());
    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    assert!(steps.iter().all(|step| {
        step.message
            .as_deref()
            .is_none_or(|m| !m.contains("listed-pw"))
    }));
    for file in database.list_downloads().await.expect("downloads") {
        assert_eq!(file.state, DownloadState::Completed);
    }
    service.shutdown().await;
}

/// Registers `file_name` in `destination` as a completed download of `package_id`.
async fn seed_completed_file(database: &Database, package_id: PackageId, file_name: &str) {
    let file = database
        .create_download(NewDownload {
            id: DownloadId::new(),
            package_id,
            source: format!("https://example.test/{file_name}")
                .parse()
                .expect("URL"),
            file_name: file_name.to_owned(),
            total_bytes: None,
            expected_checksum: None,
            account_id: None,
            proxy_profile_id: None,
            auth_profile: rd_core::AuthProfileSelection::Auto,
            initial_state: DownloadState::Queued,
            kind: rd_core::DownloadKind::Http,
            media: None,
            remote_credential_id: None,
            replay: None,
            mirror_group: None,
            enrichment: Vec::new(),
            secret_fragment: None,
        })
        .await
        .expect("download");
    for state in [
        DownloadState::Resolving,
        DownloadState::Downloading,
        DownloadState::Verifying,
        DownloadState::Completed,
    ] {
        database
            .transition_download(file.id, state)
            .await
            .expect("transition");
    }
}

/// A completed package holding `payload.zip` plus an `.sfv` index carrying `crc32` for it.
async fn seed_sfv_package(
    database: &Database,
    destination: &std::path::Path,
    crc32: &str,
    category_id: Option<rd_core::CategoryId>,
) -> PackageId {
    std::fs::create_dir_all(destination).expect("destination");
    write_zip(
        &destination.join("payload.zip"),
        SimpleFileOptions::default(),
        "payload.txt",
    );
    std::fs::write(
        destination.join("release.sfv"),
        format!("; release index\npayload.zip {crc32}\n"),
    )
    .expect("SFV index");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "checked".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    seed_completed_file(database, package.id, "payload.zip").await;
    seed_completed_file(database, package.id, "release.sfv").await;
    package.id
}

/// CRC32 of the archive the package fixture writes, so the index can match it.
fn payload_zip_crc32(destination: &std::path::Path) -> String {
    let bytes = std::fs::read(destination.join("payload.zip")).expect("archive");
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&bytes);
    format!("{:08x}", hasher.finalize())
}

#[tokio::test]
async fn a_matching_sfv_index_lets_the_package_complete() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("dl");
    // The fixture is written twice: once to learn the CRC32, once with the matching index.
    let package_id = seed_sfv_package(&database, &destination, "00000000", None).await;
    let crc32 = payload_zip_crc32(&destination);
    std::fs::write(
        destination.join("release.sfv"),
        format!("; release index\npayload.zip {crc32}\n"),
    )
    .expect("SFV index");

    run_extraction(&database, temp.path(), package_id).await;

    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let sfv = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::Sfv)
        .expect("SFV step");
    assert_eq!(sfv.state, PostprocessState::Completed);
    assert_eq!(sfv.message.as_deref(), Some("checked=1 skipped=0"));
    assert!(destination.join("payload.txt").exists());
}

#[tokio::test]
async fn a_failed_sfv_check_skips_unpacking_and_fails_the_package() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("dl");
    let package_id = seed_sfv_package(&database, &destination, "deadbeef", None).await;

    let state = run_extraction_to_end(&database, temp.path(), package_id).await;

    assert_eq!(state, rd_core::PackageState::Failed);
    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let sfv = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::Sfv)
        .expect("SFV step");
    assert_eq!(sfv.state, PostprocessState::Failed);
    let message = sfv.message.as_deref().expect("failure message");
    assert!(message.contains("mismatch=1"), "{message}");
    assert!(message.contains("payload.zip"), "{message}");
    // Extraction never ran, so nothing was unpacked and no outcome was recorded.
    assert!(!destination.join("payload.txt").exists());
    assert!(
        steps
            .iter()
            .all(|step| step.state != PostprocessState::Completed
                || step.kind == PostprocessKind::Sfv)
    );
    let package = database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == package_id)
        .expect("package");
    assert_eq!(package.extraction_result, None);
}

#[tokio::test]
async fn a_category_override_switches_the_sfv_check_off() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let root = database
        .create_storage_root(
            rd_core::StorageRootId::new(),
            rd_db::NewStorageRoot {
                name: "Downloads".to_owned(),
                path: temp.path().to_string_lossy().into_owned(),
                is_default: true,
                minimum_free_bytes: None,
            },
        )
        .await
        .expect("root");
    let category = database
        .create_category(rd_db::NewCategory {
            name: "Unchecked".to_owned(),
            color: "#38BDF8".to_owned(),
            storage_root_id: root.id,
            relative_path: "unchecked".to_owned(),
            is_default: false,
            postprocess_level: None,
            script: None,
            cleanup_extensions: None,
            recursive_unpack: None,
            sfv_verify: Some(false),
            safe_postproc: None,
            delete_par2: None,
            upload_enabled: None,
            upload_remote: None,
        })
        .await
        .expect("category");
    let destination = temp.path().join("dl");
    // A deliberately wrong checksum: the package must still complete, because the category
    // switches the check off even though the global setting has it on.
    let package_id = seed_sfv_package(&database, &destination, "deadbeef", Some(category.id)).await;

    run_extraction(&database, temp.path(), package_id).await;

    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    assert!(steps.iter().all(|step| step.kind != PostprocessKind::Sfv));
    assert!(destination.join("payload.txt").exists());
}

/// A completed Usenet package holding `payload.zip` beside a `release.par2` whose bytes are
/// not a PAR2 packet at all, plus the matching `.vol` volume.
///
/// This is the reported case reduced to its bones: intact archives beside recovery data
/// nothing can read. Nothing about the payload is wrong, and until RD-104-04 the package was
/// unreachable anyway — no unpack, no cleanup, no plugin steps, and no way to ask for them.
async fn seed_usenet_package_with_a_broken_par2(
    database: &Database,
    root: &std::path::Path,
) -> (PackageId, std::path::PathBuf) {
    let file = |subject: &str| rd_db::NewNzbFile {
        subject: subject.to_owned(),
        poster: "fixture".to_owned(),
        groups: vec!["alt.binaries.test".to_owned()],
        segments: vec![rd_db::NewNzbSegment {
            number: 1,
            bytes: 128,
            message_id: format!("{subject}@example.test"),
        }],
    };
    let import = database
        .add_nzb_import(rd_db::NewNzbImport {
            name: "release.nzb".to_owned(),
            sha256: "ab".repeat(32),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: rd_core::ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![
                file("payload.zip"),
                file("release.par2"),
                file("release.vol000+01.par2"),
            ],
        })
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            root.to_path_buf(),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue as package");
    assert_eq!(package.kind, rd_core::DownloadKind::Usenet);
    let destination = std::path::PathBuf::from(&package.destination);
    std::fs::create_dir_all(&destination).expect("destination");
    write_zip(
        &destination.join("payload.zip"),
        SimpleFileOptions::default(),
        "payload.txt",
    );
    std::fs::write(destination.join("release.par2"), b"not a PAR2 packet").expect("index");
    std::fs::write(
        destination.join("release.vol000+01.par2"),
        b"nor is this one",
    )
    .expect("volume");
    complete_every_download(database, package.id).await;
    (package.id, destination)
}

/// Walks every download row of a package to `Completed`.
///
/// A postponed recovery volume starts at `Skipped` (RD-107-04), so the walk begins one step
/// earlier for those rows: the fixtures that predate the postponement want the whole set on
/// disk, and saying so explicitly is what keeps them testing what they were written for.
async fn complete_every_download(database: &Database, package_id: PackageId) {
    for download in database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|download| download.package_id == package_id)
    {
        let path: &[DownloadState] = if download.state == DownloadState::Skipped {
            &[
                DownloadState::Queued,
                DownloadState::Resolving,
                DownloadState::Downloading,
                DownloadState::Verifying,
                DownloadState::Completed,
            ]
        } else {
            &[
                DownloadState::Resolving,
                DownloadState::Downloading,
                DownloadState::Verifying,
                DownloadState::Completed,
            ]
        };
        for state in path {
            database
                .transition_download(download.id, *state)
                .await
                .expect("transition");
        }
    }
}

#[tokio::test]
async fn an_unreadable_par2_index_is_reported_against_the_whole_set() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (package_id, destination) =
        seed_usenet_package_with_a_broken_par2(&database, &temp.path().join("usenet")).await;

    // Careful post-processing is the default, so this run still stops at the failure.
    let state = run_extraction_to_end(&database, temp.path(), package_id).await;
    assert_eq!(state, rd_core::PackageState::Failed);
    assert!(!destination.join("payload.txt").exists());

    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let par2 = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::Par2)
        .expect("PAR2 step");
    assert_eq!(par2.state, PostprocessState::Failed);
    let message = par2.message.as_deref().unwrap_or_default();
    // Both members of the set were tried before the package was given up on.
    assert!(
        message.contains("2 file(s) of this set"),
        "the sibling volume was never tried: {message}"
    );
}

#[tokio::test]
async fn a_broken_par2_no_longer_locks_a_package_that_is_asked_to_post_process_anyway() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (package_id, destination) =
        seed_usenet_package_with_a_broken_par2(&database, &temp.path().join("usenet")).await;

    let state =
        run_extraction_with(&database, temp.path(), package_id, ExtractionTrigger::Force).await;

    assert_eq!(state, rd_core::PackageState::Completed);
    assert!(
        destination.join("payload.txt").exists(),
        "the archive was intact all along and should have been unpacked"
    );
}

#[tokio::test]
async fn switching_safe_post_processing_off_unpacks_despite_the_failed_repair() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "safe_postproc": false }),
        )
        .await
        .expect("settings");
    let (package_id, destination) =
        seed_usenet_package_with_a_broken_par2(&database, &temp.path().join("usenet")).await;

    let state = run_extraction_to_end(&database, temp.path(), package_id).await;

    assert_eq!(state, rd_core::PackageState::Completed);
    assert!(destination.join("payload.txt").exists());
    // The failure is still on the record; it just no longer decides everything after it.
    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    assert_eq!(
        steps
            .iter()
            .find(|step| step.kind == PostprocessKind::Par2)
            .expect("PAR2 step")
            .state,
        PostprocessState::Failed
    );
}

#[tokio::test]
async fn a_package_without_rar_volumes_plans_no_integrity_test() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("dl");
    // The substitute check follows the archives, not the package kind: this fixture is a ZIP
    // beside an `.sfv` index, so there is no RAR set to test and no step is recorded at all.
    let package_id = seed_sfv_package(&database, &destination, "00000000", None).await;
    let crc32 = payload_zip_crc32(&destination);
    std::fs::write(
        destination.join("release.sfv"),
        format!("; release index\npayload.zip {crc32}\n"),
    )
    .expect("SFV index");

    run_extraction(&database, temp.path(), package_id).await;

    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    assert!(
        !steps
            .iter()
            .any(|step| step.kind == PostprocessKind::RarTest),
        "a package without RAR volumes has nothing to test: {steps:?}"
    );
}

/// The reason a folder rename has to rewrite `postprocess_steps.source_path` (RD-106-13).
///
/// `source_path` is not a note about a step, it is the step's identity: the table is keyed by
/// `(owner_id, kind, source_path)` and `steps::find_step` looks a step up by exactly that. Left
/// pointing at the folder the package no longer has, a second run — and `extract/force` exists
/// to start one on a finished package — would not recognise its own earlier work and would
/// write a second set of rows beside the first, so "has this package been repaired yet?" would
/// start answering wrongly.
#[tokio::test]
async fn a_renamed_package_folder_does_not_give_a_second_run_a_second_set_of_steps() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let destination = temp.path().join("library").join("Old Name");
    let package_id = seed_nested_package(&database, &destination, 1).await;
    run_extraction(&database, temp.path(), package_id).await;

    let before = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    assert!(
        !before.is_empty(),
        "the first run recorded nothing to rename"
    );

    // Rename the way the endpoint does: the row first, the disk after.
    let renamed = temp.path().join("library").join("New Name");
    database
        .rename_package_directory(
            package_id,
            "New Name".to_owned(),
            renamed.to_string_lossy().into_owned(),
        )
        .await
        .expect("rename")
        .expect("package");
    std::fs::rename(&destination, &renamed).expect("move the folder");

    let after_rename = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    assert_eq!(
        after_rename.len(),
        before.len(),
        "the rename itself must not add or drop a step"
    );
    let old_prefix = destination.to_string_lossy().into_owned();
    for step in &after_rename {
        assert!(
            !step.source_path.starts_with(&old_prefix),
            "a step still points into the old folder: {}",
            step.source_path
        );
        assert!(
            step.output_path
                .as_deref()
                .is_none_or(|path| !path.starts_with(&old_prefix)),
            "a step's output still points into the old folder: {:?}",
            step.output_path
        );
    }

    // `run_extraction_with` would return at once here: the package is already `Completed`, so
    // its wait loop is satisfied before the pipeline has done anything. The forced run is
    // therefore driven by hand and waited on by the mark the last step leaves behind.
    let mark = chrono::Utc::now();
    let service = ExtractionService::start(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
        },
    );
    service
        .request(package_id, ExtractionTrigger::Force)
        .await
        .expect("force");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            // Cleanup is the last step of the pipeline, so its fresh timestamp means every
            // unpack row the run was going to write has been written.
            let done = database
                .list_postprocess_steps(&package_id.to_string())
                .await
                .expect("steps")
                .iter()
                .any(|step| step.kind == PostprocessKind::Cleanup && step.updated_at > mark);
            if done {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the forced run finished");
    service.shutdown().await;

    let after_force = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let duplicated: Vec<&str> = after_force
        .iter()
        .filter(|step| step.source_path.starts_with(&old_prefix))
        .map(|step| step.source_path.as_str())
        .collect();
    assert!(
        duplicated.is_empty(),
        "the forced run resurrected rows under the old folder: {duplicated:?}"
    );
    assert_eq!(
        after_force.len(),
        after_rename.len(),
        "the forced run wrote a second set of steps instead of finding its own: {:?}",
        after_force
            .iter()
            .map(|step| (step.kind, step.source_path.as_str()))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// RD-107-04: postponing the recovery volumes and the way back into downloading
// ---------------------------------------------------------------------------

/// One PAR2 packet: the magic, the total length, the MD5 of everything after it, then that
/// payload (the recovery set id, the packet type and the type's own body).
///
/// The digest is passed in rather than computed: this crate has no MD5 implementation and is
/// not going to grow one for a fixture. A payload edited without its digest is skipped by the
/// parser, which makes the whole index unreadable and fails the test loudly rather than
/// quietly testing something else.
fn par2_packet(payload: &[u8], digest: [u8; 16]) -> Vec<u8> {
    let mut packet = Vec::from(*b"PAR2\0PKT");
    let length = 32_u64 + payload.len() as u64;
    packet.extend_from_slice(&length.to_le_bytes());
    packet.extend_from_slice(&digest);
    packet.extend_from_slice(payload);
    packet
}

/// A genuinely parseable PAR2 index describing one 12-byte file in 4-byte slices.
///
/// The file it describes, `damaged.bin`, is deliberately never written: a missing file is
/// three missing blocks, the index itself carries no recovery packets and no volume is on
/// disk, so `rust_par2` reports the one verdict this job is about — `NotEnoughBlocks` with
/// three needed and none available. Two packets are enough for that: `Main` supplies the
/// slice size the parser insists on, `FileDesc` the file and its size.
fn par2_index_bytes() -> Vec<u8> {
    const SET_ID: &[u8; 16] = b"RDTESTSET0000001";
    const FILE_ID: &[u8; 16] = b"RDTESTFILE000001";
    let mut main = Vec::new();
    main.extend_from_slice(SET_ID);
    main.extend_from_slice(b"PAR 2.0\0Main\0\0\0\0");
    main.extend_from_slice(&4_u64.to_le_bytes()); // slice size
    main.extend_from_slice(&1_u32.to_le_bytes()); // number of files
    main.extend_from_slice(FILE_ID);
    let mut description = Vec::new();
    description.extend_from_slice(SET_ID);
    description.extend_from_slice(b"PAR 2.0\0FileDesc");
    description.extend_from_slice(FILE_ID);
    description.extend_from_slice(&[0x11; 16]); // full-file MD5, never reached
    description.extend_from_slice(&[0x22; 16]); // first-16K MD5, never reached
    description.extend_from_slice(&12_u64.to_le_bytes());
    description.extend_from_slice(b"damaged.bin\0"); // padded to a multiple of four

    let mut index = par2_packet(
        &main,
        [
            0x23, 0x30, 0x8c, 0x6e, 0x64, 0x4e, 0x71, 0xfd, 0xea, 0xcb, 0xc9, 0xf9, 0xb7, 0xb4,
            0x76, 0xcb,
        ],
    );
    index.extend_from_slice(&par2_packet(
        &description,
        [
            0x11, 0x4d, 0xd6, 0x69, 0x9b, 0x09, 0x66, 0x01, 0x0b, 0x22, 0x90, 0xae, 0x77, 0x40,
            0x5f, 0x6a,
        ],
    ));
    index
}

/// A queued Usenet package whose PAR2 index reports a three-block gap.
///
/// The payload and the index arrive; the three recovery volumes stay where enqueueing put
/// them, which is `Skipped`.
async fn seed_usenet_package_short_of_blocks(
    database: &Database,
    root: &std::path::Path,
) -> (PackageId, std::path::PathBuf) {
    let file = |subject: &str| rd_db::NewNzbFile {
        subject: subject.to_owned(),
        poster: "fixture".to_owned(),
        groups: vec!["alt.binaries.test".to_owned()],
        segments: vec![rd_db::NewNzbSegment {
            number: 1,
            bytes: 128,
            message_id: format!("{subject}@example.test"),
        }],
    };
    let import = database
        .add_nzb_import(rd_db::NewNzbImport {
            name: "short.nzb".to_owned(),
            sha256: "cd".repeat(32),
            category_id: None,
            source: rd_core::IngressSource::Manual,
            priority: None,
            import_mode: rd_core::ImportMode::Enqueue,
            source_path: None,
            password: None,
            announce_arrival: false,
            files: vec![
                file("payload.zip"),
                file("release.par2"),
                file("release.vol000+01.par2"),
                file("release.vol001+02.par2"),
                file("release.vol003+16.par2"),
            ],
        })
        .await
        .expect("NZB import");
    let package = database
        .enqueue_nzb_import(
            import.id,
            root.to_path_buf(),
            rd_core::DownloadPriority::Normal,
            false,
        )
        .await
        .expect("enqueue as package");
    let destination = std::path::PathBuf::from(&package.destination);
    std::fs::create_dir_all(&destination).expect("destination");
    write_zip(
        &destination.join("payload.zip"),
        SimpleFileOptions::default(),
        "payload.txt",
    );
    std::fs::write(destination.join("release.par2"), par2_index_bytes()).expect("index");
    // Only what really came down: the postponed volumes stay `Skipped`.
    for download in database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|download| download.package_id == package.id)
        .filter(|download| download.state != DownloadState::Skipped)
    {
        for state in [
            DownloadState::Resolving,
            DownloadState::Downloading,
            DownloadState::Verifying,
            DownloadState::Completed,
        ] {
            database
                .transition_download(download.id, state)
                .await
                .expect("transition");
        }
    }
    (package.id, destination)
}

/// The pipeline internals, without the job loop: these tests assert what one pass leaves
/// behind, and the interesting pass is the one that does *not* end in a terminal state.
fn extraction_inner(database: &Database, temp: &std::path::Path) -> std::sync::Arc<crate::Inner> {
    // The receiver is dropped straight away: these tests call the pipeline directly and
    // nothing ever queues another job through this handle.
    let (jobs, _) = tokio::sync::mpsc::channel(8);
    std::sync::Arc::new(crate::Inner {
        database: database.clone(),
        config: ExtractionConfig {
            default_passwords_file: temp.join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: temp.join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
        },
        hold: rd_core::PostprocessHold::new(),
        jobs,
        in_flight: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        shutdown: tokio_util::sync::CancellationToken::new(),
        plugin_steps: None,
        storage: None,
    })
}

async fn package_state(database: &Database, package_id: PackageId) -> rd_core::PackageState {
    database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == package_id)
        .expect("package")
        .state
}

/// The heart of RD-107-04: a repair short of blocks re-queues what it needs and stands down.
///
/// Before this there was no way back: the scheduler considered the package downloaded, the
/// pipeline was already running, and nothing could move a package from post-processing into
/// downloading again. The proof that the way back exists is the package state — it is
/// `Downloading`, not `Failed` — together with the two volumes that left `Skipped`.
#[tokio::test]
async fn a_repair_short_of_blocks_requeues_what_it_needs_and_sends_the_package_back() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (package_id, _destination) =
        seed_usenet_package_short_of_blocks(&database, &temp.path().join("usenet")).await;
    let inner = extraction_inner(&database, temp.path());

    crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto)
        .await
        .expect("one pass");

    assert_eq!(
        package_state(&database, package_id).await,
        rd_core::PackageState::Downloading,
        "the package has to go back to downloading, which is the return path this job is about"
    );
    let states: Vec<(String, DownloadState)> = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package_id)
        .map(|file| (file.file_name, file.state))
        .collect();
    let state_of = |name: &str| {
        states
            .iter()
            .find(|(file_name, _)| file_name == name)
            .map(|(_, state)| *state)
            .unwrap_or_else(|| panic!("no row for {name}"))
    };
    // Three blocks are missing; one and two cover them, and the sixteen-block volume would
    // have cost eight times the bytes for the same repair.
    assert_eq!(
        state_of("release.vol000+01.par2"),
        DownloadState::Queued,
        "the one-block volume should have been re-queued"
    );
    assert_eq!(
        state_of("release.vol001+02.par2"),
        DownloadState::Queued,
        "the two-block volume should have been re-queued"
    );
    assert_eq!(
        state_of("release.vol003+16.par2"),
        DownloadState::Skipped,
        "nothing beyond the gap may be fetched"
    );

    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let par2 = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::Par2)
        .expect("PAR2 step");
    assert_eq!(par2.state, PostprocessState::Queued, "waiting, not failed");
    assert_eq!(
        par2.code.as_deref(),
        Some(crate::par2_refill::AWAITING_BLOCKS)
    );
}

/// While the re-queued volumes are on their way, the pipeline does not run again.
///
/// Recovery after a restart lands here: `ExtractionService::recover` re-requests every package
/// with a queued step, and without this guard that second pass would verify the same gap and
/// plan against volumes that are already coming.
#[tokio::test]
async fn a_package_waiting_for_its_volumes_does_not_run_the_pipeline_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (package_id, _destination) =
        seed_usenet_package_short_of_blocks(&database, &temp.path().join("usenet")).await;
    let inner = extraction_inner(&database, temp.path());
    crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto)
        .await
        .expect("first pass");

    // Exactly what a restart does.
    crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto)
        .await
        .expect("second pass");

    assert_eq!(
        package_state(&database, package_id).await,
        rd_core::PackageState::Downloading
    );
    let queued = database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package_id)
        .filter(|file| rd_core::is_par2_volume(&file.file_name))
        .filter(|file| file.state == DownloadState::Queued)
        .count();
    assert_eq!(
        queued, 2,
        "the second pass must not order the same gap covered twice"
    );
}

/// With nothing left to fetch, the shortfall is a verdict — and it carries a stable code.
#[tokio::test]
async fn a_shortfall_with_nothing_left_to_fetch_fails_with_a_stable_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let (package_id, _destination) =
        seed_usenet_package_short_of_blocks(&database, &temp.path().join("usenet")).await;
    // Every postponed volume is gone, as it would be for a set that really is too small.
    for download in database
        .list_downloads()
        .await
        .expect("downloads")
        .into_iter()
        .filter(|file| file.package_id == package_id)
        .filter(|file| file.state == DownloadState::Skipped)
    {
        database
            .transition_download(download.id, DownloadState::Cancelled)
            .await
            .expect("transition");
    }
    let inner = extraction_inner(&database, temp.path());

    crate::package_job::run_package(&inner, package_id, ExtractionTrigger::Auto)
        .await
        .expect("one pass");

    assert_eq!(
        package_state(&database, package_id).await,
        rd_core::PackageState::Failed
    );
    let steps = database
        .list_postprocess_steps(&package_id.to_string())
        .await
        .expect("steps");
    let par2 = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::Par2)
        .expect("PAR2 step");
    assert_eq!(par2.state, PostprocessState::Failed);
    assert_eq!(
        par2.code.as_deref(),
        Some(crate::par2_job::NOT_ENOUGH_BLOCKS),
        "a package that cannot be repaired has to say so in a way four languages can read"
    );
    assert_eq!(par2.params.get("needed").map(String::as_str), Some("3"));
    assert_eq!(par2.params.get("available").map(String::as_str), Some("0"));
}

/// RD-108-08: an unpack failure names itself in the `code` field, and the text stays text.
///
/// Both halves matter. The code is what four catalogues translate, and the message is what a
/// build that does not know the code yet still shows — so a code that travelled inside the
/// message, the way extraction reported until now, was one wire format too many.
#[tokio::test]
async fn an_unpack_failure_carries_its_code_in_the_field_and_not_in_the_message() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let directory = temp.path().join("package");
    std::fs::create_dir_all(&directory).expect("package folder");
    let broken = directory.join("release.zip");
    std::fs::write(&broken, b"this is not a ZIP").expect("archive");
    let rar = directory.join("other.rar");
    std::fs::write(&rar, b"this is not a RAR either").expect("archive");
    let owner = PackageId::new().to_string();
    let inner = extraction_inner(&database, temp.path());
    let sets = vec![
        rd_postprocess::ArchiveSet {
            kind: rd_files::ArchiveKind::Zip,
            base: "release".to_owned(),
            volumes: vec![broken],
        },
        rd_postprocess::ArchiveSet {
            kind: rd_files::ArchiveKind::Rar,
            base: "other".to_owned(),
            volumes: vec![rar],
        },
    ];
    let context = crate::unpack_job::UnpackContext {
        owner: &owner,
        directory: &directory,
        downloads: &[],
        candidates: &[None],
        limits: rd_postprocess::ArchiveLimits::default(),
        rar_tool: None,
        rar_conflict: Some("rar_tool=unrar, rar_executable=7z".to_owned()),
        delete_volumes: false,
        trigger: ExtractionTrigger::Manual,
    };

    let ok = crate::unpack_job::run(&inner, &context, &[], &sets)
        .await
        .expect("one unpack pass");

    assert!(!ok, "neither set can be unpacked");
    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    let zip = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::ExtractZip)
        .expect("ZIP step");
    assert_eq!(zip.state, PostprocessState::Failed);
    assert_eq!(zip.code.as_deref(), Some("extract.failed"));
    let message = zip.message.as_deref().unwrap_or_default();
    assert!(
        !message.contains("extract."),
        "the code belongs in the field, not in the text: {message}"
    );
    assert!(
        !zip.params
            .get("detail")
            .unwrap_or(&String::new())
            .is_empty(),
        "the tool's own words travel as the detail parameter"
    );

    // The misconfiguration that stops a RAR set before it starts reports the same way.
    let rar = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::ExtractRar)
        .expect("RAR step");
    assert_eq!(rar.state, PostprocessState::Failed);
    assert_eq!(rar.code.as_deref(), Some("extract.tool_mismatch"));
    assert_eq!(
        rar.params.get("detail").map(String::as_str),
        Some("rar_tool=unrar, rar_executable=7z")
    );
    assert!(
        !rar.message
            .as_deref()
            .unwrap_or_default()
            .contains("extract."),
        "{:?}",
        rar.message
    );
}
