//! The `RETR` half of a transfer: opening the data connection and closing it again.
//!
//! Written generically over the stream type rather than behind the [`crate::client::Connection`]
//! enum: `retr_as_stream` hands back a `DataStream<T>` whose type differs between plain FTP and
//! FTPS, so a dispatching wrapper would have to box it. Keeping this generic lets both flavours
//! share one implementation.
//!
//! The bytes themselves are moved by [`Staging::stream`], which `rd-sftp` drives too.

use std::time::Duration;

use anyhow::Result;
use rd_limits::ScopedLimiter;
use suppaftp::FtpResult;
use suppaftp::tokio::{ImplAsyncFtpStream, TokioTlsStream};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use rd_transfer_file::{Staging, TransferEnd};

/// Streams `path` into the staging file.
///
/// The offset must already have been established with `REST`; this function does not send it,
/// because a server that silently ignores `REST` would otherwise have its restart written
/// on top of the existing partial file.
pub async fn retrieve<T>(
    stream: &mut ImplAsyncFtpStream<T>,
    path: &str,
    staging: &Staging<'_>,
    sink: &mut tokio::fs::File,
    limiter: &ScopedLimiter,
    cancellation: &CancellationToken,
    read_timeout: Duration,
) -> Result<FtpResult<TransferEnd>>
where
    T: TokioTlsStream + Send + Sync + 'static,
{
    let mut data = match stream.retr_as_stream(path).await {
        Ok(data) => data,
        Err(error) => return Ok(Err(error)),
    };
    let outcome = staging
        .stream(&mut data, sink, limiter, cancellation, read_timeout)
        .await?;
    // The data connection has to be closed and its control response read either way;
    // skipping it on a stop would leave the control channel out of step for the next
    // command on this connection.
    let finalized = stream.finalize_retr_stream(data).await;
    match (&outcome, finalized) {
        // A stopped transfer ends with an "aborted" control response, which is expected
        // and must not be reported as a failure.
        (TransferEnd::Stopped, _) => Ok(Ok(TransferEnd::Stopped)),
        (TransferEnd::Complete, Err(error)) => Ok(Err(error)),
        (TransferEnd::Complete, Ok(())) => {
            sink.flush().await?;
            Ok(Ok(TransferEnd::Complete))
        }
    }
}
