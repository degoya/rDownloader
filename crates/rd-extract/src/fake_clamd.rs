//! A `clamd` stand-in for the tests (RD-190-14): `zPING`, `zVERSION` and `zINSTREAM` on a local
//! TCP port, with a verdict by content — the EICAR test string is "found", anything else is
//! clean.
//!
//! The EICAR string itself is assembled at run time by [`eicar`] and never stands in a source
//! file, so no checkout of this repository is ever a virus scanner's finding.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// What the stand-in recognises: the middle of the EICAR string.
const MARKER: &[u8] = b"EICAR-STANDARD-ANTIVIRUS-TEST-FILE";

/// The EICAR test file, put together from three parts so no source line carries it whole.
pub(crate) fn eicar() -> Vec<u8> {
    [
        r"X5O!P%@AP[4\PZX54(P^)7CC)7}$",
        "EICAR-STANDARD-ANTIVIRUS",
        "-TEST-FILE!$H+H*",
    ]
    .concat()
    .into_bytes()
}

pub(crate) struct FakeClamd {
    /// `127.0.0.1:<port>`, as the settings take it.
    pub(crate) address: String,
    scans: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeClamd {
    /// Starts one with clamd's default `StreamMaxLength` of 25 MiB.
    pub(crate) async fn start() -> Self {
        Self::with_stream_limit(25 * 1024 * 1024).await
    }

    /// Starts one that refuses a stream longer than `limit`, as clamd does.
    pub(crate) async fn with_stream_limit(limit: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address").to_string();
        let scans = Arc::new(AtomicUsize::new(0));
        let counter = scans.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let counter = counter.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, limit, &counter).await;
                });
            }
        });
        Self {
            address,
            scans,
            task,
        }
    }

    /// How many streams were scanned to the end.
    pub(crate) fn scans(&self) -> usize {
        self.scans.load(Ordering::SeqCst)
    }
}

impl Drop for FakeClamd {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(mut stream: TcpStream, limit: usize, scans: &AtomicUsize) -> std::io::Result<()> {
    let mut command = Vec::new();
    loop {
        let byte = stream.read_u8().await?;
        if byte == 0 || command.len() > 64 {
            break;
        }
        command.push(byte);
    }
    match command.as_slice() {
        b"zPING" => stream.write_all(b"PONG\0").await,
        b"zVERSION" => stream.write_all(b"ClamAV 1.5.4/27780/fake\0").await,
        b"zINSTREAM" => {
            let mut data = Vec::new();
            loop {
                let length = stream.read_u32().await? as usize;
                if length == 0 {
                    break;
                }
                let start = data.len();
                data.resize(start + length, 0);
                stream.read_exact(&mut data[start..]).await?;
                if data.len() > limit {
                    stream
                        .write_all(b"INSTREAM size limit exceeded. ERROR\0")
                        .await?;
                    // Read what is still coming before closing: closing on unread data resets
                    // the connection, and the reset can overtake the answer just written.
                    let mut sink = [0_u8; 64 * 1024];
                    while stream.read(&mut sink).await? > 0 {}
                    return Ok(());
                }
            }
            scans.fetch_add(1, Ordering::SeqCst);
            if data.windows(MARKER.len()).any(|window| window == MARKER) {
                stream
                    .write_all(b"stream: Eicar-Test-Signature FOUND\0")
                    .await
            } else {
                stream.write_all(b"stream: OK\0").await
            }
        }
        _ => stream.write_all(b"UNKNOWN COMMAND\0").await,
    }
}
