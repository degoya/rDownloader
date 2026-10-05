//! The unpack: the password list, the codes a failure carries, and an archive tool below its
//! security floor.

use std::time::Duration;

use rd_core::{DownloadId, DownloadState, PackageId, PostprocessKind, PostprocessState};
use rd_db::{Database, NewDownload, NewPackage, PackageChange};
use zip::write::SimpleFileOptions;

#[cfg(unix)]
use crate::tests::{run_extraction_to_end, seed_completed_file};
use crate::{
    ExtractionConfig, ExtractionService, ExtractionTrigger,
    tests::{extraction_inner, write_zip},
};

#[tokio::test]
async fn manual_extraction_uses_package_password_list_and_deletes_originals() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    // The package password below lives in the vault (RD-190-04).
    database
        .install_file_vault(temp.path().join("secrets"))
        .await
        .expect("vault");
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
            serde_json::json!({ "default_level": "delete" }),
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
    let stored = database
        .list_packages_with_passwords()
        .await
        .expect("packages");
    assert!(stored[0].has_password);
    // RD-104-04: an archive password is readable, not just countable.
    assert_eq!(stored[0].password.as_deref(), Some("unused-package-pw"));
    // The extraction reads it out of the vault (RD-190-04).
    assert_eq!(
        database
            .package_password(package.id)
            .await
            .expect("package password")
            .as_deref(),
        Some("unused-package-pw")
    );

    let service = ExtractionService::start(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: std::env::temp_dir().join("rd-scripts-test"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
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
        direct: &[],
        candidates: &[None],
        limits: rd_postprocess::ArchiveLimits::default(),
        rar_tool: None,
        rar_conflict: Some("rar_tool=unrar, rar_executable=7z".to_owned()),
        rar_outdated: None,
        delete_volumes: false,
        trigger: ExtractionTrigger::Manual,
        target: crate::unpack_job::UnpackTarget::Package,
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

/// Security review 2026-09-28, finding 5: a RAR set whose only tool is below its security
/// floor fails with the code that names the tool, the version it reported and the floor, and
/// the tool is never started.
#[tokio::test]
async fn an_outdated_archive_tool_fails_the_unpack_with_its_version_and_the_floor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let directory = temp.path().join("package");
    std::fs::create_dir_all(&directory).expect("package folder");
    let rar = directory.join("release.rar");
    std::fs::write(&rar, b"not read: the tool is refused first").expect("archive");
    let owner = PackageId::new().to_string();
    let inner = extraction_inner(&database, temp.path());
    let sets = vec![rd_postprocess::ArchiveSet {
        kind: rd_files::ArchiveKind::Rar,
        base: "release".to_owned(),
        volumes: vec![rar],
    }];
    let context = crate::unpack_job::UnpackContext {
        owner: &owner,
        directory: &directory,
        downloads: &[],
        direct: &[],
        candidates: &[None],
        limits: rd_postprocess::ArchiveLimits::default(),
        rar_tool: None,
        rar_conflict: None,
        rar_outdated: Some(crate::settings::OutdatedTool {
            tool: "unrar",
            found: "6.11".to_owned(),
            minimum: "6.12".to_owned(),
        }),
        delete_volumes: false,
        trigger: ExtractionTrigger::Manual,
        target: crate::unpack_job::UnpackTarget::Package,
    };

    let ok = crate::unpack_job::run(&inner, &context, &[], &sets)
        .await
        .expect("one unpack pass");

    assert!(!ok, "an outdated tool is a failure, not a skipped step");
    let steps = database
        .list_postprocess_steps(&owner)
        .await
        .expect("steps");
    let step = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::ExtractRar)
        .expect("RAR step");
    assert_eq!(step.state, PostprocessState::Failed);
    assert_eq!(step.code.as_deref(), Some(crate::unpack_job::TOOL_OUTDATED));
    assert_eq!(step.params.get("tool").map(String::as_str), Some("unrar"));
    assert_eq!(step.params.get("found").map(String::as_str), Some("6.11"));
    assert_eq!(step.params.get("minimum").map(String::as_str), Some("6.12"));
}

/// The same through the whole pipeline: the package's settings name an `unrar` 6.11, and the
/// tool is asked for its banner but never started on the archive - no integrity test, no
/// extraction - while the unpack step names the version and the floor.
#[cfg(unix)]
#[tokio::test]
async fn a_package_never_starts_an_archive_tool_below_its_security_floor() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    let tools = temp.path().join("tools");
    std::fs::create_dir_all(&tools).expect("tools");
    let unrar = tools.join("unrar");
    let started = tools.join("started-on-an-archive");
    std::fs::write(
        &unrar,
        format!(
            "#!/bin/sh\n[ $# -eq 0 ] && {{ echo 'UNRAR 6.11 freeware      Copyright (c) 1993-2022'; exit 0; }}\n\
             touch '{}'\nexit 1\n",
            started.display()
        ),
    )
    .expect("fake unrar");
    std::fs::set_permissions(&unrar, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({ "rar_tool": "unrar", "rar_executable": unrar }),
        )
        .await
        .expect("settings");
    let destination = temp.path().join("dl");
    std::fs::create_dir_all(&destination).expect("destination");
    std::fs::write(
        destination.join("release.rar"),
        b"Rar!\x1a\x07\x01\x00 not a real set",
    )
    .expect("archive");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "outdated".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    seed_completed_file(&database, package.id, "release.rar").await;

    run_extraction_to_end(&database, temp.path(), package.id).await;

    assert!(!started.exists(), "unrar 6.11 was started on the archive");
    let steps = database
        .list_postprocess_steps(&package.id.to_string())
        .await
        .expect("steps");
    let unpack = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::ExtractRar)
        .expect("RAR step");
    assert_eq!(unpack.state, PostprocessState::Failed, "{steps:?}");
    assert_eq!(
        unpack.code.as_deref(),
        Some(crate::unpack_job::TOOL_OUTDATED)
    );
    assert_eq!(unpack.params.get("found").map(String::as_str), Some("6.11"));
    assert!(
        steps
            .iter()
            .filter(|step| step.kind == PostprocessKind::RarTest)
            .all(|step| step.state == PostprocessState::Skipped),
        "{steps:?}"
    );
}
