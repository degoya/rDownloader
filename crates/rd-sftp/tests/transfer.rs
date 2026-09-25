//! End-to-end tests of the SFTP runner against the in-process SSH fixture.

mod fixture_server;

use std::sync::Arc;

use fixture_server::{Behaviour, Fixture, RemoteFile};
use rd_core::{
    DownloadKind, DownloadState, PackageId, RemoteAuthMode, RemoteProtocol, RemoteTarget,
};
use rd_db::{Database, NewDownload, NewPackage, NewRemoteCredential};
use rd_scheduler::{ExternalRunner, RunOutcome};
use rd_sftp::{SftpRunner, SftpService};
use russh::keys::PrivateKey;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

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

/// Two fixed ed25519 identities, so "the server was rebuilt with a different host key" is
/// an explicit swap between two known keys rather than a random draw. They are test
/// fixtures with no access to anything and are deliberately checked in.
const HOST_KEY_A: &str = include_str!("fixtures/host_key_a");
const HOST_KEY_B: &str = include_str!("fixtures/host_key_b");

fn host_key() -> PrivateKey {
    parse_host_key(HOST_KEY_A)
}

fn other_host_key() -> PrivateKey {
    parse_host_key(HOST_KEY_B)
}

fn parse_host_key(pem: &str) -> PrivateKey {
    russh::keys::decode_secret_key(pem, None).expect("fixture host key")
}

struct Harness {
    _directory: tempfile::TempDir,
    database: Database,
    service: SftpService,
    fixture: Fixture,
    destination: std::path::PathBuf,
    settings: Arc<RwLock<rd_core::RemoteSettings>>,
}

impl Harness {
    async fn start() -> Self {
        Self::start_with(Fixture::start(host_key()).await).await
    }

    async fn start_with(fixture: Fixture) -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let database = Database::open(directory.path().join("sftp.sqlite3"))
            .await
            .expect("database");
        let secrets = rd_secrets::SecretStore::open(directory.path().join("secrets"))
            .await
            .expect("secrets");
        let settings = Arc::new(RwLock::new(rd_core::RemoteSettings::default()));
        let service = SftpService::new(database.clone(), secrets, settings.clone());
        let destination = directory.path().join("downloads");
        tokio::fs::create_dir_all(&destination).await.expect("dir");
        Self {
            _directory: directory,
            database,
            service,
            fixture,
            destination,
            settings,
        }
    }

    /// Turns on first-use trust, which is off by default.
    async fn allow_first_use_trust(&self) {
        self.settings.write().await.remote_ssh_auto_trust = true;
    }

    async fn credential(&self) -> rd_core::RemoteCredentialId {
        self.database
            .create_remote_credential(NewRemoteCredential {
                name: "fixture".to_owned(),
                protocol: RemoteProtocol::Sftp,
                host: "127.0.0.1".to_owned(),
                port: self.fixture.port,
                username: Some("tester".to_owned()),
                auth_mode: RemoteAuthMode::Password,
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

    async fn queue(&self, remote_path: &str, name: &str) -> rd_core::DownloadFile {
        let package_id = PackageId::new();
        self.database
            .create_package(NewPackage {
                id: package_id,
                name: "sftp".to_owned(),
                destination: self.destination.to_string_lossy().into_owned(),
                category_id: None,
                priority: rd_core::DownloadPriority::Normal,
                postprocess_level: None,
                script: None,
                enrichment: Vec::new(),
            })
            .await
            .expect("package");
        let source = format!("sftp://127.0.0.1:{}{remote_path}", self.fixture.port);
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
                kind: DownloadKind::Sftp,
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

    fn runner(&self) -> SftpRunner {
        SftpRunner::new(self.service.clone())
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

    async fn run(&self, file: &rd_core::DownloadFile) -> RunOutcome {
        self.runner()
            .run(
                file,
                &self.package_of(file).await,
                CancellationToken::new(),
                run_limits(),
            )
            .await
            .expect("run")
    }

    async fn reload(&self, file: &rd_core::DownloadFile) -> rd_core::DownloadFile {
        self.database
            .get_download(file.id)
            .await
            .expect("reload")
            .expect("row")
    }

    fn part_path(&self, file: &rd_core::DownloadFile) -> std::path::PathBuf {
        self.destination
            .join(".rdownloader")
            .join(format!("{}.part", file.id))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_host_key_blocks_the_transfer_and_reports_its_fingerprint() {
    // Default settings: nothing is trusted until a person says so.
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    match harness.run(&file).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::HOST_KEY_UNKNOWN));
            let fingerprint = failure.params.get("fingerprint").expect("fingerprint");
            assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
            assert_eq!(
                failure.params.get("algorithm").map(String::as_str),
                Some("ssh-ed25519")
            );
        }
        other => panic!("expected the host key to block the transfer, got {other:?}"),
    }
    assert!(!harness.destination.join("movie.bin").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confirmed_host_key_lets_the_transfer_through() {
    let harness = Harness::start().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    // First attempt records the fingerprint the user has to confirm.
    let RunOutcome::Failed(blocked) = harness.run(&file).await else {
        panic!("an unknown key must block first");
    };
    let fingerprint = blocked.params.get("fingerprint").expect("fingerprint");
    harness
        .database
        .trust_ssh_host_key(rd_core::SshHostKey {
            host: "127.0.0.1".to_owned(),
            port: harness.fixture.port,
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: fingerprint.clone(),
            first_seen: chrono::Utc::now(),
        })
        .await
        .expect("trust");

    let outcome = harness.run(&harness.reload(&file).await).await;
    assert!(
        matches!(outcome, RunOutcome::Completed { .. }),
        "{outcome:?}"
    );
    let written = tokio::fs::read(harness.destination.join("movie.bin"))
        .await
        .expect("final file");
    assert_eq!(written, payload());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_host_key_blocks_even_though_first_use_trust_is_on() {
    // The endpoint is trusted, but for a *different* key than the one the server now
    // offers. That is what a rebuilt server and an active interception both look like from
    // the client, so it must block regardless of the first-use setting.
    let harness = Harness::start_with(Fixture::start(other_host_key()).await).await;
    harness.allow_first_use_trust().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));

    // Fingerprint of the *other* identity, recorded as the trusted one for this endpoint.
    let impostor = russh::keys::decode_secret_key(HOST_KEY_A, None).expect("key");
    let stored_fingerprint = impostor
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();
    harness
        .database
        .trust_ssh_host_key(rd_core::SshHostKey {
            host: "127.0.0.1".to_owned(),
            port: harness.fixture.port,
            algorithm: "ssh-ed25519".to_owned(),
            fingerprint: stored_fingerprint.clone(),
            first_seen: chrono::Utc::now(),
        })
        .await
        .expect("trust");

    let file = harness.queue("/srv/movie.bin", "movie.bin").await;
    match harness.run(&file).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::HOST_KEY_CHANGED));
            assert_eq!(
                failure.params.get("stored_fingerprint"),
                Some(&stored_fingerprint)
            );
            assert_ne!(
                failure.params.get("fingerprint"),
                Some(&stored_fingerprint),
                "the offered key must differ from the stored one"
            );
        }
        other => panic!("a changed host key must block, got {other:?}"),
    }
    assert!(!harness.destination.join("movie.bin").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_transfer_resumes_from_the_partial_file() {
    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let first = harness.run(&file).await;
    assert!(matches!(first, RunOutcome::Failed(_)), "{first:?}");
    let partial = tokio::fs::metadata(harness.part_path(&file))
        .await
        .expect("partial kept");
    assert_eq!(partial.len(), 50_000);

    harness.fixture.set_behaviour(Behaviour::default());
    let second = harness.run(&harness.reload(&file).await).await;
    assert!(matches!(second, RunOutcome::Completed { .. }), "{second:?}");
    let written = tokio::fs::read(harness.destination.join("movie.bin"))
        .await
        .expect("final file");
    // The whole payload, not the tail appended to a restarted download.
    assert_eq!(written, payload());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_remote_file_blocks_the_resume_and_keeps_the_partial() {
    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness.fixture.put(
        "/srv/movie.bin",
        RemoteFile::new(payload()).modified_at(1_767_268_800),
    );
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    harness.fixture.set_behaviour(Behaviour {
        truncate_after: Some(50_000),
        ..Behaviour::default()
    });
    let _ = harness.run(&file).await;

    // Same size, different modification time: the file was replaced.
    harness.fixture.set_behaviour(Behaviour::default());
    harness.fixture.put(
        "/srv/movie.bin",
        RemoteFile::new(payload()).modified_at(1_800_000_000),
    );

    match harness.run(&harness.reload(&file).await).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::FILE_CHANGED));
        }
        other => panic!("expected a refused resume, got {other:?}"),
    }
    assert!(harness.part_path(&file).exists(), "the partial is kept");
    assert!(!harness.destination.join("movie.bin").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_password_reports_an_auth_failure() {
    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(payload()));
    harness.fixture.set_behaviour(Behaviour {
        reject_password: true,
        ..Behaviour::default()
    });
    let file = harness.queue("/srv/movie.bin", "movie.bin").await;

    match harness.run(&file).await {
        RunOutcome::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::AUTH_FAILED));
            // Retrying a password the server keeps refusing is pointless.
            assert!(!failure.category.is_retryable());
        }
        other => panic!("expected an auth failure, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_directory_is_walked_and_a_single_file_is_not() {
    let harness = Harness::start().await;
    harness.allow_first_use_trust().await;
    harness.fixture.put_directory("/srv");
    harness.fixture.put_directory("/srv/extras");
    harness
        .fixture
        .put("/srv/movie.bin", RemoteFile::new(vec![0u8; 10]));
    harness
        .fixture
        .put("/srv/extras/notes.txt", RemoteFile::new(vec![1u8; 20]));
    let credential = harness.credential().await;

    let directory = RemoteTarget::parse(
        &format!("sftp://127.0.0.1:{}/srv", harness.fixture.port)
            .parse()
            .expect("url"),
    )
    .expect("target");
    let (probed, used) = harness
        .service
        .probe(&directory, Some(credential))
        .await
        .expect("probe");
    assert_eq!(used, Some(credential));
    let listing = match probed {
        rd_sftp::Probed::Resolved(listing) => listing,
        rd_sftp::Probed::Failed(failure) => panic!("probe failed: {failure:?}"),
    };
    assert!(!listing.single_file);
    let paths: Vec<&str> = listing
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(paths, ["extras", "extras/notes.txt", "movie.bin"]);
    // SFTP reads from an explicit offset, so resume never depends on a server capability.
    assert!(listing.supports_resume);

    let single = RemoteTarget::parse(
        &format!("sftp://127.0.0.1:{}/srv/movie.bin", harness.fixture.port)
            .parse()
            .expect("url"),
    )
    .expect("target");
    let (probed, _) = harness
        .service
        .probe(&single, Some(credential))
        .await
        .expect("probe");
    let listing = match probed {
        rd_sftp::Probed::Resolved(listing) => listing,
        rd_sftp::Probed::Failed(failure) => panic!("probe failed: {failure:?}"),
    };
    assert!(listing.single_file);
    assert_eq!(listing.root, "/srv");
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].path, "movie.bin");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_without_a_stored_login_says_so() {
    let harness = Harness::start().await;
    let target = RemoteTarget::parse(
        &"sftp://unconfigured.example/srv/a.bin"
            .parse()
            .expect("url"),
    )
    .expect("target");

    let (probed, used) = harness.service.probe(&target, None).await.expect("probe");
    assert!(used.is_none());
    match probed {
        rd_sftp::Probed::Failed(failure) => {
            assert_eq!(failure.code.as_deref(), Some(rd_sftp::NO_CREDENTIAL));
            assert_eq!(
                failure.params.get("host").map(String::as_str),
                Some("unconfigured.example")
            );
        }
        rd_sftp::Probed::Resolved(_) => panic!("there is no server to resolve against"),
    }
}
