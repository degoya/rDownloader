//! RD-109-45 — a package still named after its hoster takes the name the resolver learns.
//!
//! The reported case: a single `1fichier.com` link with no path segment and no check result.
//! Intake has nothing but the host to name the package after, so the package is `1fichier.com`
//! and the file `download.bin`. The resolver learns the real name moments later, and until now
//! that name reached only the download row.
//!
//! The folder is the delicate half. These cases pin both halves of the decision: for a fresh
//! transfer the rename happens *before* the destination is ever created, so there is nothing on
//! disk to move; when an earlier attempt did leave a folder behind, the existing two-phase move
//! carries it over; and a folder that is already taken stops the rename instead of merging into
//! somebody else's data.

use std::path::{Path, PathBuf};

use rd_core::DownloadFile;
use rd_scheduler::{FileSpec, PackageSpec, SchedulerConfig, SchedulerHandle};

const LINK: &str = "https://1fichier.com/?8x6wertoi51r8vptrojn";
const RESOLVED: &str = "outlander.s08e01.german.bdrip.x264-intention.rar";
const RELEASE: &str = "outlander.s08e01.german.bdrip.x264-intention";

async fn scheduler_over(directory: &Path) -> (SchedulerHandle, rd_db::Database) {
    let database = rd_db::Database::open(directory.join("naming-test.sqlite3"))
        .await
        .expect("database");
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = SchedulerHandle::start(
        database.clone(),
        SchedulerConfig::for_directory(directory.join("downloads")),
        secrets,
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    (scheduler, database)
}

/// One paused file below `storage/`, in a package named `package_name`.
async fn paused_package(
    directory: &Path,
    package_name: &str,
) -> (SchedulerHandle, rd_db::Database, DownloadFile, PathBuf) {
    let (scheduler, database) = scheduler_over(directory).await;
    let spec = PackageSpec {
        name: package_name.to_owned(),
        destination: directory.join("storage"),
        category_id: None,
        priority: rd_core::DownloadPriority::default(),
        password: None,
        start_paused: true,
        postprocess_level: None,
        script: None,
        enrichment: Vec::new(),
    };
    let files = vec![FileSpec {
        source: LINK.parse().expect("url"),
        file_name: "download.bin".to_owned(),
        size: None,
        account_id: None,
        proxy_profile_id: None,
        auth_profile: rd_core::AuthProfileSelection::Auto,
        kind: rd_core::DownloadKind::Http,
        media: None,
        remote_credential_id: None,
        replay: None,
        mirror_group: None,
        skipped: false,
        enrichment: Vec::new(),
        secret_fragment: None,
    }];
    let (_package, files) = scheduler
        .enqueue_package(spec, files)
        .await
        .expect("enqueue");
    let file = files.into_iter().next().expect("one file");
    let destination = directory.join("storage").join(package_name);
    (scheduler, database, file, destination)
}

async fn package_of(database: &rd_db::Database, file: &DownloadFile) -> rd_core::DownloadPackage {
    database
        .list_packages()
        .await
        .expect("packages")
        .into_iter()
        .find(|package| package.id == file.package_id)
        .expect("package")
}

#[tokio::test]
async fn a_hoster_named_package_takes_the_name_the_resolver_learned() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, database, file, old) = paused_package(temporary.path(), "1fichier.com").await;

    scheduler
        .adopt_resolved_package_name(&file, RESOLVED)
        .await
        .expect("adopt");

    let package = package_of(&database, &file).await;
    assert_eq!(package.name, RELEASE, "the package kept the hoster name");
    assert_eq!(
        PathBuf::from(&package.destination),
        old.parent().expect("base").join(RELEASE),
        "the folder did not follow the name"
    );
    // Nothing was on disk, so nothing is outstanding: a sweep of a folder that was never
    // created would look like an unfinished move for the rest of the package's life.
    assert_eq!(
        database
            .package_previous_destination(file.package_id)
            .await
            .expect("previous"),
        None,
        "a move was recorded although no folder existed"
    );
}

#[tokio::test]
async fn a_package_somebody_named_keeps_its_name() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, database, file, _) =
        paused_package(temporary.path(), "Outlander Staffel 8").await;

    scheduler
        .adopt_resolved_package_name(&file, RESOLVED)
        .await
        .expect("adopt");

    assert_eq!(
        package_of(&database, &file).await.name,
        "Outlander Staffel 8",
        "a name that is not the hoster's was overwritten"
    );
}

#[tokio::test]
async fn a_folder_that_is_already_there_stops_the_rename() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, database, file, _) = paused_package(temporary.path(), "1fichier.com").await;
    let taken = temporary.path().join("storage").join(RELEASE);
    tokio::fs::create_dir_all(&taken).await.expect("folder");
    tokio::fs::write(taken.join("somebody-elses.mkv"), b"mine")
        .await
        .expect("payload");

    scheduler
        .adopt_resolved_package_name(&file, RESOLVED)
        .await
        .expect("adopt");

    assert_eq!(
        package_of(&database, &file).await.name,
        "1fichier.com",
        "the package moved into a folder that was already taken"
    );
    assert!(
        taken.join("somebody-elses.mkv").exists(),
        "the data in the taken folder did not survive"
    );
}

#[tokio::test]
async fn a_folder_an_earlier_attempt_left_behind_is_carried_over() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (scheduler, database, file, old) = paused_package(temporary.path(), "1fichier.com").await;
    tokio::fs::create_dir_all(&old).await.expect("old folder");
    tokio::fs::write(old.join(&file.file_name), b"partial")
        .await
        .expect("payload");

    scheduler
        .adopt_resolved_package_name(&file, RESOLVED)
        .await
        .expect("adopt");

    let package = package_of(&database, &file).await;
    assert_eq!(package.name, RELEASE, "the package kept the hoster name");
    let new = PathBuf::from(&package.destination);
    assert!(
        new.join(&file.file_name).exists(),
        "the data was not carried over to {}",
        new.display()
    );
    assert!(!old.exists(), "the hoster-named folder was left behind");
    assert_eq!(
        database
            .package_previous_destination(file.package_id)
            .await
            .expect("previous"),
        None,
        "the finished move is still recorded as outstanding"
    );
}
