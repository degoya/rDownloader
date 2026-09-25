//! End-to-end tests of the FTP runner against the in-process fixture server.

mod fixture_server;

use std::sync::Arc;

use fixture_server::{Behaviour, Fixture, RemoteFile};
use rd_core::{
    DownloadKind, DownloadState, PackageId, RemoteAuthMode, RemoteProtocol, RemoteTarget,
};
use rd_db::{Database, NewDownload, NewPackage, NewRemoteCredential};
use rd_ftp::{FtpRunner, FtpService};
use rd_scheduler::{ExternalRunner, RunOutcome};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// The payload used throughout; large enough to span several read chunks.
/// Unlimited pacing plus one request slot; the queue supplies these in production.
fn run_limits() -> rd_scheduler::RunLimits {
    rd_scheduler::RunLimits {
        max_parallel_requests: 1,
        bandwidth: rd_limits::ScopedLimiter::unlimited(),
    }
}

fn payload() -> Vec<u8> {
    (0..200_000u32).map(|index| (index % 251) as u8).collect()
}

struct Harness {
    _directory: tempfile::TempDir,
    database: Database,
    service: FtpService,
    fixture: Fixture,
    destination: std::path::PathBuf,
}

impl Harness {
    async fn start() -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("ftp.sqlite3"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secrets");
        let fixture = Fixture::start().await;
        let settings = Arc::new(RwLock::new(rd_core::RemoteSettings::default()));
        let service = FtpService::new(
            database.clone(),
            secrets,
            settings,
            Arc::new(RwLock::new(rd_http::NetworkDefaults::default())),
        );
        let destination = directory.path().join("downloads");
        tokio::fs::create_dir_all(&destination).await.expect("dir");
        Self {
            _directory: directory,
            database,
            service,
            fixture,
            destination,
        }
    }

    /// Stores an anonymous login for the fixture and returns its id.
    async fn credential(&self) -> rd_core::RemoteCredentialId {
        self.database
            .create_remote_credential(NewRemoteCredential {
                name: "fixture".to_owned(),
                protocol: RemoteProtocol::Ftp,
                host: "127.0.0.1".to_owned(),
                port: self.fixture.port,
                username: None,
                auth_mode: RemoteAuthMode::Anonymous,
                passive: true,
                enabled: true,
                secret_ref: None,
                key_ref: None,
                passphrase_ref: None,
            })
            .await
            .expect("credential")
            .id
    }

    /// Queues one remote file and returns the row.
    async fn queue(&self, remote_path: &str, name: &str) -> rd_core::DownloadFile {
        let package_id = PackageId::new();
        self.database
            .create_package(NewPackage {
                id: package_id,
                name: "ftp".to_owned(),
                destination: self.destination.to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let source = format!("ftp://127.0.0.1:{}{remote_path}", self.fixture.port);
        self.database
            .create_download(NewDownload {
                id: rd_core::DownloadId::new(),
                package_id,
                source: source.parse().expect("url"),
                file_name: name.to_owned(),
                total_bytes: None,
                expected_checksum: None,
                account_id: None,
                proxy_profile_id: None,
                auth_profile: rd_core::AuthProfileSelection::Auto,
                initial_state: DownloadState::Queued,
                kind: DownloadKind::Ftp,
                media: None,
                remote_credential_id: Some(self.credential().await),
                mirror_group: None,
                replay: None,
                enrichment: Vec::new(),
                secret_fragment: None,
            })
            .await
            .expect("download")
    }

    fn runner(&self) -> FtpRunner {
        FtpRunner::new(self.service.clone())
    }

    async fn package_of(&self, file: &rd_core::DownloadFile) -> rd_core::DownloadPackage {
        self.database
            .list_packages()
            .await
            .expect("packages")
            .into_iter()
            .find(|package| package.id == file.package_id)
            .expect("package")
    }

    fn part_path(&self, file: &rd_core::DownloadFile) -> std::path::PathBuf {
        self.destination
            .join(".rdownloader")
            .join(format!("{}.part", file.id))
    }
}

#[tokio::test]
async fn a_whole_file_is_downloaded_and_promoted() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/pub/movie.bin", RemoteFile::new(payload()));
    harness.fixture.put_directory("/pub");
    let file = harness.queue("/pub/movie.bin", "movie.bin").await;

    let outcome = harness
        .runner()
        .run(
            &file,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("run");

    assert!(matches!(outcome, RunOutcome::Completed { .. }));
    let written = tokio::fs::read(harness.destination.join("movie.bin"))
        .await
        .expect("final file");
    assert_eq!(written, payload());
    // The staging file must not survive a completed transfer.
    assert!(!harness.part_path(&file).exists());
}

#[tokio::test]
async fn an_interrupted_transfer_resumes_from_the_partial_file() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/pub/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/pub/movie.bin", "movie.bin").await;

    // First attempt: the server cuts the data connection after 50 000 bytes.
    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let first = harness
        .runner()
        .run(
            &file,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("first run");
    assert!(
        matches!(first, RunOutcome::Failed(_)),
        "short read must fail"
    );
    let partial = tokio::fs::metadata(harness.part_path(&file))
        .await
        .expect("partial kept");
    assert_eq!(partial.len(), 50_000);

    // Second attempt: the server behaves, and REST continues where the file left off.
    harness.fixture.set_behaviour(Behaviour::default());
    let reloaded = harness
        .database
        .get_download(file.id)
        .await
        .expect("reload")
        .expect("row");
    let second = harness
        .runner()
        .run(
            &reloaded,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("second run");
    assert!(matches!(second, RunOutcome::Completed { .. }));
    let written = tokio::fs::read(harness.destination.join("movie.bin"))
        .await
        .expect("final file");
    // The whole payload, not the tail appended to a restarted download.
    assert_eq!(written, payload());
}

#[tokio::test]
async fn a_changed_remote_file_blocks_the_resume_and_keeps_the_partial() {
    let harness = Harness::start().await;
    harness.fixture.put(
        "/pub/movie.bin",
        RemoteFile::new(payload()).modified_at("20260101120000"),
    );
    let file = harness.queue("/pub/movie.bin", "movie.bin").await;

    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let _ = harness
        .runner()
        .run(
            &file,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("first run");

    // The file is replaced on the server: same size, different timestamp.
    harness.fixture.set_behaviour(Behaviour::default());
    harness.fixture.put(
        "/pub/movie.bin",
        RemoteFile::new(payload()).modified_at("20260202080000"),
    );
    let reloaded = harness
        .database
        .get_download(file.id)
        .await
        .expect("reload")
        .expect("row");
    let outcome = harness
        .runner()
        .run(
            &reloaded,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("second run");

    match outcome {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_ftp::FILE_CHANGED));
        }
        other => panic!("expected a refused resume, got {other:?}"),
    }
    // Refusing must not delete what was already downloaded.
    assert!(harness.part_path(&file).exists());
    assert!(!harness.destination.join("movie.bin").exists());
}

#[tokio::test]
async fn a_server_without_rest_reports_that_resume_is_impossible() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/pub/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/pub/movie.bin", "movie.bin").await;

    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let _ = harness
        .runner()
        .run(
            &file,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("first run");

    harness.fixture.set_behaviour(Behaviour {
        refuse_rest: true,
        hide_rest_feature: true,
        ..Behaviour::default()
    });
    let reloaded = harness
        .database
        .get_download(file.id)
        .await
        .expect("reload")
        .expect("row");
    let outcome = harness
        .runner()
        .run(
            &reloaded,
            &harness.package_of(&file).await,
            CancellationToken::new(),
            run_limits(),
        )
        .await
        .expect("second run");

    match outcome {
        RunOutcome::Failed(failure) => {
            // The bug this guards: silently restarting would append the whole file to the
            // 50 000 bytes already on disk and produce a corrupt result.
            assert_eq!(failure.code.as_deref(), Some(rd_ftp::RESUME_UNSUPPORTED));
        }
        other => panic!("expected a refused resume, got {other:?}"),
    }
}

#[tokio::test]
async fn a_directory_is_walked_through_mlsd_and_through_list() {
    for refuse_mlsd in [false, true] {
        let harness = Harness::start().await;
        harness.fixture.put_directory("/pub");
        harness.fixture.put_directory("/pub/extras");
        harness
            .fixture
            .put("/pub/movie.bin", RemoteFile::new(vec![0u8; 10]));
        harness
            .fixture
            .put("/pub/extras/notes.txt", RemoteFile::new(vec![1u8; 20]));
        harness.fixture.set_behaviour(Behaviour {
            refuse_mlsd,
            ..Behaviour::default()
        });
        let credential = harness.credential().await;
        let target = RemoteTarget::parse(
            &format!("ftp://127.0.0.1:{}/pub", harness.fixture.port)
                .parse()
                .expect("url"),
        )
        .expect("target");

        let (probed, used) = harness
            .service
            .probe(&target, Some(credential))
            .await
            .expect("probe");
        assert_eq!(used, Some(credential));
        let listing = match probed {
            rd_ftp::Probed::Resolved(listing) => listing,
            rd_ftp::Probed::Failed(failure) => panic!("probe failed: {failure:?}"),
        };
        assert!(!listing.single_file, "a directory is not a single file");
        let paths: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(
            paths,
            ["extras", "extras/notes.txt", "movie.bin"],
            "mlsd={refuse_mlsd}"
        );
        let sizes: Vec<Option<u64>> = listing
            .entries
            .iter()
            .map(|entry| entry.size.map(rd_core::ByteCount::get))
            .collect();
        assert_eq!(sizes, [None, Some(20), Some(10)]);
    }
}

#[tokio::test]
async fn a_single_file_link_resolves_without_a_tree() {
    let harness = Harness::start().await;
    harness.fixture.put_directory("/pub");
    harness
        .fixture
        .put("/pub/movie.bin", RemoteFile::new(payload()));
    let credential = harness.credential().await;
    let target = RemoteTarget::parse(
        &format!("ftp://127.0.0.1:{}/pub/movie.bin", harness.fixture.port)
            .parse()
            .expect("url"),
    )
    .expect("target");

    let (probed, _) = harness
        .service
        .probe(&target, Some(credential))
        .await
        .expect("probe");
    let listing = match probed {
        rd_ftp::Probed::Resolved(listing) => listing,
        rd_ftp::Probed::Failed(failure) => panic!("probe failed: {failure:?}"),
    };
    assert!(listing.single_file);
    assert_eq!(listing.root, "/pub");
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].path, "movie.bin");
    assert!(
        listing.supports_resume,
        "the fixture advertises REST STREAM"
    );
}

#[tokio::test]
async fn a_link_without_a_stored_login_says_so_instead_of_failing_obscurely() {
    let harness = Harness::start().await;
    let target = RemoteTarget::parse(&"ftp://unconfigured.example/pub/a.bin".parse().expect("url"))
        .expect("target");

    let (probed, used) = harness.service.probe(&target, None).await.expect("probe");
    assert!(used.is_none());
    match probed {
        rd_ftp::Probed::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_ftp::NO_CREDENTIAL));
            assert_eq!(
                failure.params.get("host").map(String::as_str),
                Some("unconfigured.example")
            );
        }
        rd_ftp::Probed::Resolved(_) => panic!("there is no server to resolve against"),
    }
}
