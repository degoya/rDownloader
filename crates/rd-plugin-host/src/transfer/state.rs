//! Per-invocation state of one transfer attempt.
//!
//! Everything the guest is allowed to touch during a transfer is reachable only from here,
//! and every field is something the host decided: which file the bytes go into, how fast they
//! may arrive, which hosts may be dialled, and whether the download is still wanted.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use rd_core::{Failure, FailureKind};
use rd_files::PartFile;
use tokio_util::sync::CancellationToken;

use crate::manifest::NetStreamCapability;

/// Longest a single socket read or write may block before the transfer is called stalled.
pub(crate) const SOCKET_TIMEOUT: Duration = Duration::from_secs(60);
/// Connections one invocation may hold open at once. A protocol needs a control and a data
/// channel; more than that is a plugin doing something else.
pub(crate) const MAX_CONNECTIONS: usize = 4;

/// Where the bytes of this transfer go and what the host already accepted.
pub struct TransferTarget {
    /// The `.part` file inside the package's staging directory, opened by the runner.
    pub part: PartFile,
    /// Bytes already on disk, so a resumed backend is told where to continue.
    pub committed: u64,
    /// Total size when the probe reported one.
    pub total: Option<u64>,
}

/// How one attempt ended, in the host's terms.
pub enum TransferOutcome {
    /// The backend says the file is complete; the runner still verifies before promoting.
    Complete {
        committed: u64,
        checkpoint: Option<Vec<u8>>,
    },
    /// Paused or cancelled. The checkpoint is persisted and handed back next time.
    Stopped { committed: u64, checkpoint: Vec<u8> },
}

/// Progress reported by the guest, throttled and persisted by the runner.
pub type ProgressSink = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// The state a transfer invocation runs against.
pub struct TransferState {
    pub(crate) target: TransferTarget,
    pub(crate) cancellation: CancellationToken,
    pub(crate) bandwidth: rd_limits::ScopedLimiter,
    pub(crate) stream: Option<NetStreamCapability>,
    /// Whether loopback and link-local targets may be dialled. Off outside development mode:
    /// the service's own API and a cloud metadata endpoint both live there, and no transfer
    /// protocol has a reason to reach either.
    pub(crate) allow_local: bool,
    pub(crate) tls: Arc<rustls::ClientConfig>,
    pub(crate) progress: ProgressSink,
    pub(crate) open_connections: usize,
    /// Set once the sink refuses a write, so a failure cannot be reported as a completion.
    pub(crate) sink_failed: Arc<AtomicBool>,
}

impl TransferState {
    /// Builds the state for one attempt.
    #[must_use]
    pub fn new(
        target: TransferTarget,
        cancellation: CancellationToken,
        bandwidth: rd_limits::ScopedLimiter,
        stream: Option<NetStreamCapability>,
        tls: Arc<rustls::ClientConfig>,
        progress: ProgressSink,
    ) -> Self {
        Self {
            target,
            cancellation,
            bandwidth,
            stream,
            allow_local: false,
            tls,
            progress,
            open_connections: 0,
            sink_failed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Permits loopback and link-local targets, for the contract tests and for a service
    /// started with `--plugin-allow-local-targets`.
    #[must_use]
    pub fn allowing_local_targets(mut self) -> Self {
        self.allow_local = true;
        self
    }

    /// Bytes the host has accepted so far.
    #[must_use]
    pub fn committed(&self) -> u64 {
        self.target.committed
    }

    pub(crate) fn note_sink_failure(&self) {
        self.sink_failed.store(true, Ordering::SeqCst);
    }

    pub(crate) fn sink_failed(&self) -> bool {
        self.sink_failed.load(Ordering::SeqCst)
    }
}

/// Turns the guest's own verdict into the host's, refusing a completion the sink contradicts.
pub(crate) fn outcome(
    state: &TransferState,
    end: super::exports::rdownloader::plugin::transfer::TransferEnd,
) -> Result<TransferOutcome, Failure> {
    use super::exports::rdownloader::plugin::transfer::TransferEnd;
    if state.sink_failed() {
        return Err(Failure::coded(
            FailureKind::Transient {
                retry_after_seconds: None,
            },
            "plugin.sink_write_failed",
            "The transfer could not be written to disk",
        ));
    }
    match end {
        TransferEnd::Complete(checkpoint) => Ok(TransferOutcome::Complete {
            committed: state.target.committed,
            checkpoint,
        }),
        TransferEnd::Stopped(checkpoint) => Ok(TransferOutcome::Stopped {
            committed: state.target.committed,
            checkpoint,
        }),
        TransferEnd::Failed(failure) => Err(crate::component::from_wit_failure(failure)),
    }
}
