//! The checks before the unpack: an SFV index, a PAR2 set nothing can read, and the RAR
//! integrity test that runs when neither of them answered (RD-104-04).

use rd_core::{DownloadState, PackageId, PostprocessKind, PostprocessState};
use rd_db::{Database, NewPackage};
use zip::write::SimpleFileOptions;

use crate::{
    ExtractionTrigger,
    tests::{
        run_extraction, run_extraction_to_end, run_extraction_with, seed_completed_file, write_zip,
    },
};

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
    // The English text stays as the fallback; the code is what the interface translates
    // (audit 1.9.1, INTAKE-09).
    assert_eq!(sfv.code.as_deref(), Some(crate::steps::codes::SFV_VERIFIED));
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
    assert_eq!(sfv.code.as_deref(), Some(crate::steps::codes::SFV_MISMATCH));
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
            unpack_to_subfolder: None,
            unwrap_package_folder: None,
            direct_unpack: None,
            malware_scan: None,
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
