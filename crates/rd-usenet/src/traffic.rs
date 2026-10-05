//! Bytes per Usenet server: counted on the segment path, written in batches (RD-1100-05).
//!
//! The pool adds every article body it reads to its server's counter, one atomic addition and
//! nothing else, so counting costs the segment path no lock and no database round trip. A
//! flusher takes what the counters hold every [`FLUSH_INTERVAL`] and hands it to the database
//! as one transaction, and once more when the service stops. A crash loses what was counted
//! since the last flush and nothing before it (`usenet.before_traffic_flushed` in
//! `crates/rd-core/recovery-matrix.md`).
//!
//! What is counted is the article body as the server sent it, yEnc-encoded, before decoding:
//! that is what a provider meters. It is a little more than the payload — yEnc's escaping and
//! its header and trailer lines add one to three per cent — and it includes bodies that failed
//! their checksum and were asked for again, because those were delivered and billed as well.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use rd_core::UsenetServerId;
use rd_db::{Database, UsenetQuotaReached};
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

/// How often the counted bytes are written.
///
/// The most a crash loses, and the longest a used-up quota goes unnoticed. One writer command
/// per interval while anything downloads, and none while nothing does.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(10);

/// The bytes counted per server since the last flush; cheap to clone, one per process.
#[derive(Clone, Default)]
pub struct UsenetTraffic {
    counters: Arc<std::sync::Mutex<HashMap<UsenetServerId, Arc<AtomicU64>>>>,
    /// One flush at a time, so a periodic one and the last one at shutdown cannot interleave.
    flushing: Arc<Mutex<()>>,
}

impl UsenetTraffic {
    /// The counter the pool adds `server`'s bytes to; the same one for as long as the process
    /// runs, so a pool rebuilt between files keeps counting where the last one stopped.
    #[must_use]
    pub fn counter(&self, server: UsenetServerId) -> Arc<AtomicU64> {
        Arc::clone(self.lock().entry(server).or_default())
    }

    /// What has been counted and not yet written, per server with anything counted.
    #[must_use]
    pub fn pending(&self) -> Vec<(UsenetServerId, u64)> {
        self.lock()
            .iter()
            .map(|(server, counter)| (*server, counter.load(Ordering::Acquire)))
            .filter(|(_, bytes)| *bytes > 0)
            .collect()
    }

    /// Writes everything counted since the last flush and names the servers whose quota it
    /// used up.
    ///
    /// The counters are emptied before the write and given their bytes back when it fails, so
    /// a database that refused one flush loses nothing: the next one carries them.
    pub async fn flush(&self, database: &Database) -> Result<Vec<UsenetQuotaReached>> {
        let _flushing = self.flushing.lock().await;
        let counts: Vec<(UsenetServerId, u64)> = self
            .lock()
            .iter()
            .map(|(server, counter)| (*server, counter.swap(0, Ordering::AcqRel)))
            .filter(|(_, bytes)| *bytes > 0)
            .collect();
        if counts.is_empty() {
            return Ok(Vec::new());
        }
        // A stop here is a crash between counting and writing: the counts were only ever in
        // memory, and the restart starts from what the last flush wrote.
        rd_core::failpoint!("usenet.before_traffic_flushed", || anyhow::anyhow!(
            "crash point usenet.before_traffic_flushed"
        ));
        match database.record_usenet_traffic(counts.clone()).await {
            Ok(reached) => {
                for server in &reached {
                    tracing::info!(
                        server = %server.name,
                        "usenet server quota used up; its quota action applies from the next file"
                    );
                }
                Ok(reached)
            }
            Err(error) => {
                for (server, bytes) in counts {
                    self.counter(server).fetch_add(bytes, Ordering::AcqRel);
                }
                Err(error)
            }
        }
    }

    /// Flushes every `interval` until [`TrafficFlusher::shutdown`], then once more.
    #[must_use]
    pub fn start_flushing(&self, database: Database, interval: Duration) -> TrafficFlusher {
        let stop = CancellationToken::new();
        let traffic = self.clone();
        let stopped = stop.clone();
        let task = tokio::spawn(async move {
            loop {
                let last = tokio::select! {
                    () = stopped.cancelled() => true,
                    () = tokio::time::sleep(interval) => false,
                };
                if let Err(error) = traffic.flush(&database).await {
                    tracing::warn!(%error, "usenet server traffic could not be written; kept for the next flush");
                }
                if last {
                    return;
                }
            }
        });
        TrafficFlusher { stop, task }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<UsenetServerId, Arc<AtomicU64>>> {
        match self.counters.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// The running flusher; [`Self::shutdown`] writes what is left and ends it.
pub struct TrafficFlusher {
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl TrafficFlusher {
    /// Writes what was counted since the last flush and ends the task. Called after the
    /// scheduler stopped, so no article arrives after the last write.
    pub async fn shutdown(self) {
        self.stop.cancel();
        if let Err(error) = self.task.await {
            tracing::warn!(%error, "usenet traffic flusher ended abnormally");
        }
    }
}
