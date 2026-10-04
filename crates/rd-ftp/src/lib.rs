//! FTP and FTPS transfer source (RD-060-01).
//!
//! Provides the scheduler runner for `DownloadKind::Ftp` and the probe the LinkGrabber
//! uses to turn an `ftp://`/`ftps://` link into a reviewable directory listing.
//!
//! The half of a transfer that is not FTP at all — staging, resume validation, the length
//! guard and the sync before the rename — lives in `rd-transfer-file` and is shared with
//! `rd-sftp`.

mod client;
mod error;
mod listing;
mod mirror;
mod probe;
mod runner;
mod tls;
mod transfer;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use rd_core::{Failure, RemoteCredential, RemoteCredentialId, RemoteSettings, RemoteTarget};
use rd_db::Database;
use rd_http::SharedNetworkDefaults;
use rd_secrets::SecretStore;
use tokio::sync::RwLock;

pub use error::{
    AUTH_FAILED, CONNECT_FAILED, FILE_CHANGED, LISTING_TOO_LARGE, NO_CREDENTIAL, PATH_NOT_FOUND,
    RESUME_UNSUPPORTED, TLS_HANDSHAKE_FAILED, TLS_REQUIRED, UNSAFE_PATH,
};
pub use probe::Probed;
pub use runner::FtpRunner;

/// Live FTP settings shared between the API handlers and the runner.
pub use rd_transfer_file::SharedRemoteSettings;

/// Everything the probe and the runner need to reach a server.
#[derive(Clone)]
pub struct FtpService {
    database: Database,
    secrets: SecretStore,
    settings: rd_transfer_file::LiveRemoteSettings,
    network: SharedNetworkDefaults,
}

impl FtpService {
    #[must_use]
    pub fn new(
        database: Database,
        secrets: SecretStore,
        settings: SharedRemoteSettings,
        network: SharedNetworkDefaults,
    ) -> Self {
        Self {
            database,
            secrets,
            settings: rd_transfer_file::LiveRemoteSettings::new(settings),
            network,
        }
    }

    pub(crate) const fn database(&self) -> &Database {
        &self.database
    }

    /// Concurrent FTP transfers; read live so a settings change applies without a restart.
    pub(crate) fn max_parallel(&self) -> usize {
        self.settings.max_parallel()
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.settings.timeout()
    }

    /// The stored login for a target: the one pinned on the job, or the best match.
    pub(crate) async fn credential_for(
        &self,
        pinned: Option<RemoteCredentialId>,
        target: &RemoteTarget,
    ) -> Result<Result<RemoteCredential, Failure>> {
        let credential = rd_transfer_file::credential_for(&self.database, pinned, target).await?;
        Ok(probe::require_credential(credential, target))
    }

    /// Opens a logged-in control connection; held to `guard` when a stranger's document or
    /// page proposed the link (RD-150-03).
    pub(crate) async fn connect(
        &self,
        credential: &RemoteCredential,
        guard: Option<&rd_http::AddressPolicy>,
    ) -> Result<suppaftp::FtpResult<client::Connection>> {
        let password = match credential.secret_ref.as_deref() {
            Some(reference) => Some(self.secrets.get(reference).await?),
            None => None,
        };
        let custom_ca = self.network.read().await.custom_ca_pem.clone();
        client::Connection::open(
            credential,
            password.as_ref(),
            &custom_ca,
            self.timeout(),
            guard,
        )
        .await
    }

    /// Resolves one link into the listing the LinkGrabber reviews.
    ///
    /// Returns the login that reached the server alongside it, so the queue row can
    /// authenticate exactly the way the probe did. `guard` is the address rule of a link a
    /// document or a page proposed; every connection the probe opens keeps to it.
    pub async fn probe(
        &self,
        target: &RemoteTarget,
        pinned: Option<RemoteCredentialId>,
        guard: Option<&rd_http::AddressPolicy>,
    ) -> Result<(Probed, Option<RemoteCredentialId>)> {
        let credential = match self.credential_for(pinned, target).await? {
            Ok(credential) => credential,
            Err(failure) => return Ok((Probed::Failed(failure), None)),
        };
        let mut connection = match self.connect(&credential, guard).await? {
            Ok(connection) => connection,
            Err(error) => return Ok((Probed::Failed(error::classify(&error)), None)),
        };
        let probed = probe::probe(&mut connection, target).await;
        let _ = connection.quit().await;
        Ok((probed?, Some(credential.id)))
    }

    /// Checks that a stored login can reach its server and log in, for the settings UI.
    pub async fn test_credential(&self, credential: &RemoteCredential) -> Result<Option<Failure>> {
        match self.connect(credential, None).await? {
            Ok(mut connection) => {
                let _ = connection.quit().await;
                Ok(None)
            }
            Err(error) => Ok(Some(error::classify(&error))),
        }
    }
}

/// Builds the runner registered with the scheduler.
#[must_use]
pub fn build(service: FtpService) -> Arc<dyn rd_scheduler::ExternalRunner> {
    Arc::new(FtpRunner::new(service))
}

/// Loads the persisted remote-transfer settings from the shared settings blob.
pub async fn load_remote_settings(database: &Database) -> Result<RemoteSettings> {
    // Refuses a malformed blob rather than running on defaults: read once at start-up, and a
    // silently defaulted timeout or concurrency is a transfer policy nobody chose.
    Ok(database
        .service_settings::<RemoteSettings>()
        .await?
        .sanitized())
}

/// Creates the shared settings handle the runner reads.
pub async fn shared_settings(database: &Database) -> Result<SharedRemoteSettings> {
    Ok(Arc::new(RwLock::new(load_remote_settings(database).await?)))
}
