//! What the SFTP runner's end-to-end tests share: the queue-side harness around the in-process
//! fixture, the payload and the two fixed host identities.

use std::sync::Arc;

use rd_core::{DownloadKind, DownloadState, PackageId, RemoteAuthMode, RemoteProtocol};
use rd_db::{Database, NewDownload, NewPackage, NewRemoteCredential};
use rd_scheduler::{ExternalRunner, RunOutcome};
use rd_sftp::{SftpRunner, SftpService};
use russh::keys::PrivateKey;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::fixture_server::Fixture;

/// Unlimited pacing plus one request slot; the queue supplies these in production.
pub fn run_limits() -> rd_scheduler::RunLimits {
    rd_scheduler::RunLimits {
        max_parallel_requests: 1,
        bandwidth: rd_limits::ScopedLimiter::unlimited(),
        address_policy: None,
    }
}

pub fn payload() -> Vec<u8> {
    (0..200_000u32).map(|index| (index % 251) as u8).collect()
}

/// Two fixed ed25519 identities, so "the server was rebuilt with a different host key" is
/// an explicit swap between two known keys rather than a random draw. They are test
/// fixtures with no access to anything and are deliberately checked in.
pub const HOST_KEY_A: &str = include_str!("../fixtures/host_key_a");
pub const HOST_KEY_B: &str = include_str!("../fixtures/host_key_b");

pub fn host_key() -> PrivateKey {
    parse_host_key(HOST_KEY_A)
}

pub fn other_host_key() -> PrivateKey {
    parse_host_key(HOST_KEY_B)
}

pub fn parse_host_key(pem: &str) -> PrivateKey {
    russh::keys::decode_secret_key(pem, None).expect("fixture host key")
}

pub struct Harness {
    _directory: tempfile::TempDir,
    pub database: Database,
    pub service: SftpService,
    pub fixture: Fixture,
    pub destination: std::path::PathBuf,
    pub settings: Arc<RwLock<rd_core::RemoteSettings>>,
}

impl Harness {
    pub async fn start() -> Self {
        Self::start_with(Fixture::start(host_key()).await).await
    }

    pub async fn start_with(fixture: Fixture) -> Self {
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
    pub async fn allow_first_use_trust(&self) {
        self.settings.write().await.remote_ssh_auto_trust = true;
    }

    pub async fn credential(&self) -> rd_core::RemoteCredentialId {
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

    pub async fn queue(&self, remote_path: &str, name: &str) -> rd_core::DownloadFile {
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

    pub fn runner(&self) -> SftpRunner {
        SftpRunner::new(self.service.clone())
    }

    pub async fn package_of(&self, file: &rd_core::DownloadFile) -> rd_core::DownloadPackage {
        self.database
            .list_packages()
            .await
            .expect("packages")
            .into_iter()
            .find(|package| package.id == file.package_id)
            .expect("package")
    }

    pub async fn run(&self, file: &rd_core::DownloadFile) -> RunOutcome {
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

    pub async fn reload(&self, file: &rd_core::DownloadFile) -> rd_core::DownloadFile {
        self.database
            .get_download(file.id)
            .await
            .expect("reload")
            .expect("row")
    }

    pub fn part_path(&self, file: &rd_core::DownloadFile) -> std::path::PathBuf {
        self.destination
            .join(".rdownloader")
            .join(format!("{}.part", file.id))
    }
}
