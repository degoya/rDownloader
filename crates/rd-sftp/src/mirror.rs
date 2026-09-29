//! SFTP mirrors of a multi-source download (RD-150-03).
//!
//! The chunk engine asks for the file's size once and then for one reader per chunk. Each is a
//! session of its own, with the stored login that matches the mirror and the same host-key
//! trust as any SFTP link, and held to the download's address rule when its socket is opened:
//! the host is resolved once and every address checked ([`crate::client::connect`]). A chunk
//! is read from its offset with a seek, which SFTP never refuses.

use std::io::SeekFrom;

use async_trait::async_trait;
use rd_core::{Failure, FailureKind, RemoteTarget};
use rd_http::{AddressPolicy, RangeReader, RangeSource};
use tokio::io::AsyncSeekExt;

use crate::{SftpService, client::Connection, error, listing};

/// The SFTP runner's side of a multi-source download.
pub(crate) struct SftpMirror {
    service: SftpService,
}

impl SftpMirror {
    pub(crate) const fn new(service: SftpService) -> Self {
        Self { service }
    }

    async fn open(
        &self,
        target: &RemoteTarget,
        policy: Option<&AddressPolicy>,
    ) -> Result<Connection, Failure> {
        match self.service.connect(None, target, policy).await {
            Ok(connected) => connected,
            Err(error) => Err(Failure::coded(
                FailureKind::Transient {
                    retry_after_seconds: None,
                },
                error::CONNECT_FAILED,
                rd_core::redact_text(&error.to_string()),
            )),
        }
    }
}

#[async_trait]
impl RangeSource for SftpMirror {
    async fn size(
        &self,
        target: &RemoteTarget,
        policy: Option<&AddressPolicy>,
    ) -> Result<u64, Failure> {
        let connection = self.open(target, policy).await?;
        let metadata = connection
            .sftp
            .metadata(target.path.clone())
            .await
            .map_err(|error| error::classify_sftp(&error))?;
        listing::size_of(&metadata)
            .map(rd_core::ByteCount::get)
            .ok_or_else(|| {
                Failure::coded(
                    FailureKind::Permanent,
                    error::PATH_NOT_FOUND,
                    "The SFTP server did not report the file size",
                )
            })
    }

    async fn open_at(
        &self,
        target: &RemoteTarget,
        offset: u64,
        policy: Option<&AddressPolicy>,
    ) -> Result<RangeReader, Failure> {
        let connection = self.open(target, policy).await?;
        let mut file = connection
            .sftp
            .open(target.path.clone())
            .await
            .map_err(|error| error::classify_sftp(&error))?;
        if offset > 0 && file.seek(SeekFrom::Start(offset)).await.is_err() {
            return Err(error::file_changed());
        }
        Ok(Box::new(Holding {
            file,
            _connection: connection,
        }))
    }
}

/// An open remote file that keeps its SSH session up for as long as it is read.
struct Holding {
    file: russh_sftp::client::fs::File,
    _connection: Connection,
}

impl tokio::io::AsyncRead for Holding {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.file).poll_read(context, buffer)
    }
}
