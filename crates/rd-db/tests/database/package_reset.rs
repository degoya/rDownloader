//! A full package reset's rows (RD-1390-07): every file back to `queued` with what its transfer
//! produced cleared, the package's own outcome cleared, what the LinkGrabber chose kept - and
//! nothing written at all while one file is still at work.

use std::path::Path;

use rd_core::{
    DownloadId, DownloadKind, DownloadPriority, DownloadState, ExtractionResult, Failure,
    FailureKind, PackageId, PackageState, PostprocessKind, PostprocessState,
};
use rd_db::{Database, NewDownload, NewPackage};

async fn package_of(database: &Database, directory: &Path) -> (PackageId, Vec<DownloadId>) {
    let id = PackageId::new();
    database
        .create_package(NewPackage {
            id,
            name: "Broken Set".to_owned(),
            destination: directory.join("Broken Set").to_string_lossy().into_owned(),
            category_id: None,
            priority: DownloadPriority::High,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    let mut downloads = Vec::new();
    for index in 0..2 {
        let download = database
            .create_download(NewDownload {
                id: DownloadId::new(),
                package_id: id,
                source: url::Url::parse(&format!("https://files.example/{index}.rar"))
                    .expect("url"),
                file_name: format!("set.part{index}.rar"),
                total_bytes: Some(rd_core::ByteCount::new(1000).expect("bytes")),
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                initial_state: DownloadState::Queued,
                kind: DownloadKind::Http,
                media: None,
                remote_credential_id: None,
                replay: None,
                secret_fragment: None,
                mirror_group: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("download");
        downloads.push(download.id);
    }
    (id, downloads)
}

/// The first file failed, the second finished, and the unpack that followed failed.
async fn failed_package(database: &Database, directory: &Path) -> (PackageId, Vec<DownloadId>) {
    let (package, files) = package_of(database, directory).await;
    database
        .record_failure(
            files[0],
            Failure::coded(FailureKind::Offline, "http.not_found", "the file is gone"),
            None,
        )
        .await
        .expect("failure");
    for state in [
        DownloadState::Resolving,
        DownloadState::Downloading,
        DownloadState::Verifying,
    ] {
        database
            .transition_download(files[1], state)
            .await
            .expect("on its way");
    }
    database
        .complete_download(files[1], "set.part1.rar".to_owned(), None)
        .await
        .expect("complete");
    let folder = directory.join("Broken Set").to_string_lossy().into_owned();
    database
        .checkpoint_postprocess(
            package.to_string(),
            PostprocessKind::ExtractRar,
            format!("{folder}/set.part0.rar"),
            PostprocessState::Failed,
            Some(folder.clone()),
            Some("CRC error".to_owned()),
        )
        .await
        .expect("step");
    database
        .record_package_outputs(package, vec![format!("{folder}/Extras")])
        .await
        .expect("outputs");
    database
        .set_package_extraction(package, Some(ExtractionResult::Failed))
        .await
        .expect("extraction");
    database
        .set_package_state(package, PackageState::Failed, None, None, None)
        .await
        .expect("failed");
    (package, files)
}

#[tokio::test]
async fn a_package_reset_puts_every_file_and_the_package_back_to_the_start() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = rd_testkit::temp_database(directory.path()).await;
    let (package, files) = failed_package(&database, directory.path()).await;

    let reset = database.reset_package(package, true).await.expect("reset");

    assert_eq!(reset.len(), 2);
    for file in &reset {
        assert_eq!(file.state, DownloadState::Queued, "{}", file.file_name);
        assert_eq!(file.retry_count, 0);
        assert!(file.last_error.is_none(), "the error went with the reset");
        assert_eq!(file.committed_bytes.get(), 0);
        assert!(file.computed_checksum.is_none());
    }
    assert_eq!(
        reset.iter().map(|file| file.id).collect::<Vec<_>>(),
        files,
        "the files keep their order"
    );
    let row = database
        .get_package(package)
        .await
        .expect("read")
        .expect("package");
    assert_eq!(row.state, PackageState::Queued);
    assert_eq!(row.extraction_result, None, "the unpack result went");
    assert_eq!(row.postprocess.stage, None);
    assert_eq!(row.completed_at, None);
    // What the LinkGrabber chose stays.
    assert_eq!(row.name, "Broken Set");
    assert_eq!(row.priority, DownloadPriority::High);
    assert!(row.destination.ends_with("Broken Set"));
    assert!(
        database
            .list_postprocess_steps(&package.to_string())
            .await
            .expect("steps")
            .is_empty(),
        "the pipeline's steps went"
    );
    assert_eq!(
        database.package_reset_plan(package).await.expect("plan"),
        rd_db::PackageResetPlan::default(),
        "the deleted outputs are forgotten"
    );
}

#[tokio::test]
async fn a_reset_that_kept_the_unpacked_files_keeps_their_record() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = rd_testkit::temp_database(directory.path()).await;
    let (package, _files) = failed_package(&database, directory.path()).await;
    let folder = directory.path().join("Broken Set");
    let extras = format!("{}/Extras", folder.to_string_lossy());
    // Recorded twice, kept once.
    database
        .record_package_outputs(package, vec![extras.clone()])
        .await
        .expect("again");

    database.reset_package(package, false).await.expect("reset");

    let plan = database.package_reset_plan(package).await.expect("plan");
    assert_eq!(plan.unpack_outputs, vec![extras]);
}

#[tokio::test]
async fn one_file_at_work_refuses_the_whole_reset() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = rd_testkit::temp_database(directory.path()).await;
    let (package, files) = package_of(&database, directory.path()).await;
    database
        .record_failure(
            files[0],
            Failure::coded(FailureKind::Offline, "http.not_found", "the file is gone"),
            None,
        )
        .await
        .expect("failure");
    for state in [DownloadState::Resolving, DownloadState::Downloading] {
        database
            .transition_download(files[1], state)
            .await
            .expect("running");
    }

    let refused = database
        .reset_package(package, true)
        .await
        .expect_err("a running file keeps the package");

    assert_eq!(
        rd_db::store_kind(&refused),
        Some(rd_db::StoreErrorKind::WrongState)
    );
    let failed = database
        .get_download(files[0])
        .await
        .expect("read")
        .expect("row");
    assert_eq!(failed.state, DownloadState::Failed, "nothing was written");
}

#[tokio::test]
async fn an_unknown_package_is_not_found() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = rd_testkit::temp_database(directory.path()).await;

    let missing = database
        .reset_package(PackageId::new(), true)
        .await
        .expect_err("no such package");

    assert_eq!(
        rd_db::store_kind(&missing),
        Some(rd_db::StoreErrorKind::NotFound)
    );
}
