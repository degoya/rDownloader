//! The malware scan against a real `clamd` (RD-190-14): the client's three commands and a whole
//! package through the pipeline, which the stand-in of the unit tests can only imitate.
//!
//! `#[ignore]`, and failing without `RD_CLAMD_LIVE_ADDRESS`, so only CI's `clamav-live` job runs
//! them, against the pinned `clamav/clamav` service container. Locally:
//!
//! ```text
//! docker run --rm -d -p 127.0.0.1:3310:3310 clamav/clamav:1.5.4
//! RD_CLAMD_LIVE_ADDRESS=127.0.0.1:3310 cargo nextest run -p rd-extract --test clamd_live --run-ignored only
//! ```
//!
//! The EICAR test file is assembled at run time and never stands in a source file.

use std::time::Duration;

use rd_core::{DownloadId, DownloadState, PackageId, PackageState, PostprocessKind};
use rd_db::{Database, NewDownload, NewPackage};
use rd_extract::clamd::{Clamd, ClamdAddress, Verdict};
use rd_extract::{ExtractionConfig, ExtractionService, ExtractionTrigger};

fn address() -> String {
    std::env::var("RD_CLAMD_LIVE_ADDRESS")
        .expect("RD_CLAMD_LIVE_ADDRESS names the clamd these tests run against")
}

fn eicar() -> Vec<u8> {
    [
        r"X5O!P%@AP[4\PZX54(P^)7CC)7}$",
        "EICAR-STANDARD-ANTIVIRUS",
        "-TEST-FILE!$H+H*",
    ]
    .concat()
    .into_bytes()
}

fn clamd() -> Clamd {
    Clamd::new(
        ClamdAddress::parse(&address()).expect("address"),
        Duration::from_secs(60),
    )
}

#[tokio::test]
#[ignore = "needs a running clamd at RD_CLAMD_LIVE_ADDRESS"]
async fn a_real_clamd_answers_ping_version_and_finds_eicar() {
    let clamd = clamd();
    clamd.ping().await.expect("PING");
    let version = clamd.version().await.expect("VERSION");
    assert!(version.starts_with("ClamAV "), "{version}");
    match clamd.scan_reader(&eicar()[..]).await.expect("INSTREAM") {
        Verdict::Found(signature) => {
            assert!(signature.contains("Eicar"), "{signature}");
        }
        Verdict::Clean => panic!("clamd did not recognise EICAR"),
    }
    assert_eq!(
        clamd
            .scan_reader(&b"nothing to see here"[..])
            .await
            .expect("INSTREAM"),
        Verdict::Clean
    );
}

#[tokio::test]
#[ignore = "needs a running clamd at RD_CLAMD_LIVE_ADDRESS"]
async fn a_package_with_eicar_is_stopped_by_a_real_clamd() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({
                "malware_scan_enabled": true,
                "clamd_address": address(),
            }),
        )
        .await
        .expect("settings");
    let destination = temp.path().join("dl");
    std::fs::create_dir_all(&destination).expect("destination");
    std::fs::write(destination.join("eicar.com"), eicar()).expect("eicar");
    std::fs::write(destination.join("readme.txt"), b"a harmless file").expect("readme");
    let package = database
        .create_package(NewPackage {
            id: PackageId::new(),
            name: "live".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    for name in ["eicar.com", "readme.txt"] {
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

    let service = ExtractionService::start(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: temp.path().join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
    );
    service
        .request(package.id, ExtractionTrigger::Manual)
        .await
        .expect("request");
    let state = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let state = database
                .list_packages()
                .await
                .expect("packages")
                .into_iter()
                .find(|candidate| candidate.id == package.id)
                .expect("package")
                .state;
            if matches!(state, PackageState::Completed | PackageState::Failed) {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the pipeline finished");
    service.shutdown().await;

    assert_eq!(state, PackageState::Failed);
    let steps = database
        .list_postprocess_steps(&package.id.to_string())
        .await
        .expect("steps");
    let scan = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::MalwareScan)
        .expect("scan step");
    assert_eq!(
        scan.code.as_deref(),
        Some("postprocess.malware_found"),
        "{scan:?}"
    );
    assert_eq!(
        scan.params.get("file").map(String::as_str),
        Some("eicar.com")
    );
    assert!(
        destination.join("eicar.com").is_file(),
        "nothing is deleted"
    );
}
