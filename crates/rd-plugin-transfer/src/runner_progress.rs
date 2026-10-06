//! The transfer state a backend is handed, and the progress writes it starts beside the guest.

use std::sync::Arc;

use rd_plugin_host::{TransferBackend, TransferState, TransferTarget};
use rd_scheduler::RunLimits;
use tokio_util::sync::CancellationToken;

use super::{PluginTransferRunner, logged};

/// One database write per megabyte, as the native runners do: the resume-relevant state is
/// the file on disk, the row only feeds the progress bar.
const PROGRESS_INTERVAL_BYTES: u64 = 1024 * 1024;

/// The per-megabyte progress writes one transfer attempt started (RD-191-06, PLUG-19).
///
/// They run beside the guest so a slow write cannot pace the transfer, which also means one
/// can still be on its way when the attempt ends; landing after the final write, it put an
/// older count back into the row. The runner waits for them before it writes the outcome.
#[derive(Default)]
pub(super) struct ProgressWrites {
    pub(super) pending: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl ProgressWrites {
    pub(super) fn spawn(&self, write: impl std::future::Future<Output = ()> + Send + 'static) {
        let handle = tokio::spawn(write);
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|handle| !handle.is_finished());
            pending.push(handle);
        }
    }

    /// Waits for every write started so far.
    pub(super) async fn settle(&self) {
        let pending = self
            .pending
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default();
        for handle in pending {
            if let Err(error) = handle.await {
                tracing::warn!(error = %error, "a plugin transfer progress write did not finish");
            }
        }
    }
}

impl PluginTransferRunner {
    pub(super) fn state(
        &self,
        backend: &TransferBackend,
        file: rd_core::DownloadId,
        target: TransferTarget,
        cancellation: &CancellationToken,
        limits: &RunLimits,
    ) -> (TransferState, Arc<ProgressWrites>) {
        let committed = target.committed;
        let database = self.database.clone();
        let writes = Arc::new(ProgressWrites::default());
        let spawner = Arc::clone(&writes);
        let reported = Arc::new(std::sync::atomic::AtomicU64::new(committed));
        // One row update per megabyte and never on the guest's thread: the resume-relevant
        // state is the file on disk, so a slow write here must not pace the transfer.
        let progress = Arc::new(move |committed: u64, total: Option<u64>| {
            let previous = reported.load(std::sync::atomic::Ordering::Relaxed);
            if committed.saturating_sub(previous) < PROGRESS_INTERVAL_BYTES {
                return;
            }
            reported.store(committed, std::sync::atomic::Ordering::Relaxed);
            let database = database.clone();
            spawner.spawn(async move {
                logged(
                    database.set_download_progress(file, committed, total).await,
                    "record a plugin transfer's progress",
                );
            });
        });
        let state = TransferState::new(
            target,
            cancellation.clone(),
            limits.bandwidth.clone(),
            backend.manifest().capabilities.net_stream.clone(),
            Arc::clone(&self.tls),
            progress,
        );
        let state = if self.backends.allows_local_targets() {
            state.allowing_local_targets()
        } else {
            state
        };
        (state, writes)
    }
}
