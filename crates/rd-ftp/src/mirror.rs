//! FTP mirrors of a multi-source download (RD-150-03).
//!
//! The chunk engine asks for the file's size once and then for one reader per chunk. Each is a
//! connection of its own, logged in with the stored login that matches the mirror (anonymous
//! when none does) and held to the download's address rule when it is opened: the host is
//! resolved once, every address is checked, and a `PASV` reply cannot send the data connection
//! anywhere else ([`crate::client::Connection::open`]).

use async_trait::async_trait;
use rd_core::{Failure, FailureKind, RemoteTarget};
use rd_http::{AddressPolicy, RangeReader, RangeSource};

use crate::{
    FtpService,
    client::Connection,
    error::{self, classify},
};

/// The FTP runner's side of a multi-source download.
pub(crate) struct FtpMirror {
    service: FtpService,
}

impl FtpMirror {
    pub(crate) const fn new(service: FtpService) -> Self {
        Self { service }
    }

    async fn open(
        &self,
        target: &RemoteTarget,
        policy: Option<&AddressPolicy>,
    ) -> Result<Connection, Failure> {
        let credential = match self.service.credential_for(None, target).await {
            Ok(Ok(credential)) => credential,
            Ok(Err(failure)) => return Err(failure),
            Err(error) => return Err(internal(&error)),
        };
        match self.service.connect(&credential, policy).await {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(error)) => Err(classify(&error)),
            Err(error) => Err(internal(&error)),
        }
    }
}

#[async_trait]
impl RangeSource for FtpMirror {
    async fn size(
        &self,
        target: &RemoteTarget,
        policy: Option<&AddressPolicy>,
    ) -> Result<u64, Failure> {
        let mut connection = self.open(target, policy).await?;
        let size = connection.size(&target.path).await;
        let _ = connection.quit().await;
        size.map(|size| size as u64)
            .map_err(|error| classify(&error))
    }

    async fn open_at(
        &self,
        target: &RemoteTarget,
        offset: u64,
        policy: Option<&AddressPolicy>,
    ) -> Result<RangeReader, Failure> {
        let mut connection = self.open(target, policy).await?;
        if offset > 0 {
            let offset = usize::try_from(offset).map_err(|_| error::resume_unsupported())?;
            if let Err(refused) = connection.resume_from(offset).await {
                let _ = connection.quit().await;
                return Err(match &refused {
                    // A refused REST means this mirror cannot serve a chunk that does not start
                    // at the beginning of the file.
                    suppaftp::FtpError::UnexpectedResponse(_) => error::resume_unsupported(),
                    other => classify(other),
                });
            }
        }
        connection
            .into_reader(&target.path)
            .await
            .map_err(|error| classify(&error))
    }
}

/// A local failure (the store, the vault) while preparing a mirror connection.
fn internal(error: &anyhow::Error) -> Failure {
    Failure::coded(
        FailureKind::Transient {
            retry_after_seconds: None,
        },
        error::CONNECT_FAILED,
        rd_core::redact_text(&error.to_string()),
    )
}
