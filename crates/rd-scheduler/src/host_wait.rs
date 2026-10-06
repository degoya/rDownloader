//! Files that wait for a connection to their host before they start (RD-1130-02).
//!
//! The per-host connection limit (`rd_http::HostLimits`, six by default) used to be met only
//! inside the engine: the dispatcher started a file, the file took one of the
//! `max_active_files` places, and then all of its chunks queued at the limiter until another
//! file of the same host let a connection go. The row said *Downloading* with 0 B, the log said
//! nothing, and a file of another host that could have run sat queued behind it.
//!
//! The dispatch pass now asks first. A queued HTTP file starts only while its host has a free
//! connection that no file started before it is about to take; otherwise it stays `Queued`,
//! the pass goes on to the next file, and the file is listed as waiting for that host. A file
//! that runs already and whose further chunks wait holds connections and moves bytes; it is
//! left alone. The limit itself is unchanged: this only reads it.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use rd_core::{DownloadFile, DownloadId, DownloadKind};

use crate::SchedulerHandle;

/// How long a file handed to the engine still counts as starting: until its chunks have
/// queued at the limiter, which the next dispatch pass (two a second) then reads directly.
const HANDOVER_GRACE: Duration = Duration::from_secs(1);

/// What a queued HTTP file asks of its host's connections.
pub(crate) struct HostClaim {
    /// The host as the limiter counts it (`rd_core::host_key`).
    pub(crate) host: String,
    /// Connections the host could open right now.
    pub(crate) free: usize,
    /// Connections the file will ask for: its planned chunks.
    pub(crate) connections: usize,
}

/// A started file whose chunks have not reached the limiter yet.
struct Starting {
    host: String,
    connections: usize,
    /// When the worker handed the transfer to the engine; `None` while it still resolves,
    /// connects or probes.
    handed_over: Option<Instant>,
}

/// The dispatcher's view of the host limit, kept under the active set's lock.
#[derive(Default)]
pub(crate) struct HostAdmission {
    starting: HashMap<DownloadId, Starting>,
    /// Queued files the last dispatch pass held back, with the host each waits for.
    waiting: HashMap<DownloadId, String>,
}

impl HostAdmission {
    /// Whether `claim`'s host has a connection left once the files started before it on the
    /// same host take theirs. The limiter cannot know those yet: they resolve, connect and
    /// probe before their chunks ask, so three files of one host started in one pass used to
    /// see the same six free connections.
    pub(crate) fn has_room(&self, claim: &HostClaim, now: Instant) -> bool {
        let promised: usize = self
            .starting
            .values()
            .filter(|start| start.host == claim.host && !start.expired(now))
            .map(|start| start.connections)
            .sum();
        claim.free > promised
    }

    pub(crate) fn start(&mut self, id: DownloadId, claim: HostClaim) {
        self.starting.insert(
            id,
            Starting {
                host: claim.host,
                connections: claim.connections,
                handed_over: None,
            },
        );
    }

    /// The file's chunks are about to ask the limiter themselves.
    fn handed_over(&mut self, id: DownloadId, now: Instant) {
        if let Some(start) = self.starting.get_mut(&id) {
            start.handed_over = Some(now);
        }
    }

    /// The attempt ended, however it ended.
    pub(crate) fn finished(&mut self, id: &DownloadId) {
        self.starting.remove(id);
    }

    /// Takes the waits one dispatch pass found, saying in the log which began and which ended.
    pub(crate) fn settle(&mut self, waiting: HashMap<DownloadId, String>, now: Instant) {
        self.starting.retain(|_, start| !start.expired(now));
        for (id, host) in &waiting {
            if !self.waiting.contains_key(id) {
                tracing::debug!(download_id = %id, %host, "download waits for a connection to its host");
            }
        }
        for (id, host) in &self.waiting {
            if !waiting.contains_key(id) {
                tracing::debug!(download_id = %id, %host, "download no longer waits for a connection to its host");
            }
        }
        self.waiting = waiting;
    }
}

impl Starting {
    fn expired(&self, now: Instant) -> bool {
        self.handed_over
            .is_some_and(|since| now.saturating_duration_since(since) >= HANDOVER_GRACE)
    }
}

impl SchedulerHandle {
    /// What `file` asks of its host's connections, or `None` when the host limit does not
    /// apply: another kind of download, the limit switched off, an address without a host.
    pub(crate) fn host_claim(&self, file: &DownloadFile) -> Option<HostClaim> {
        if file.kind != DownloadKind::Http {
            return None;
        }
        let free = self.host_limits().free(&file.source)?;
        let host = rd_core::host_key(file.source.host_str()?);
        let budget = self.chunk_budget(&file.source);
        // A size known from the link check plans as the engine will; an unknown one is
        // learnt only by the probe, which then splits the file as far as the budget allows.
        let connections = file.total_bytes.map_or(budget, |total| {
            rd_http::plan_chunks(Some(total.get()), true, budget).len()
        });
        Some(HostClaim {
            host,
            free,
            connections: connections.max(1),
        })
    }

    /// The worker hands the transfer of `id` to the engine.
    pub(crate) async fn host_handed_over(&self, id: DownloadId) {
        self.active
            .lock()
            .await
            .host
            .handed_over(id, Instant::now());
    }

    /// Queued downloads held back because their host has no free connection, with the host
    /// each waits for (RD-1130-02). Runtime state only, as of the last dispatch pass.
    pub async fn host_waits(&self) -> HashMap<DownloadId, String> {
        self.active.lock().await.host.waiting.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use rd_core::DownloadId;

    use super::{HANDOVER_GRACE, HostAdmission, HostClaim};

    fn claim(host: &str, free: usize, connections: usize) -> HostClaim {
        HostClaim {
            host: host.to_owned(),
            free,
            connections,
        }
    }

    #[test]
    fn files_started_on_one_host_promise_its_connections_until_their_chunks_ask() {
        let now = Instant::now();
        let mut admission = HostAdmission::default();
        assert!(admission.has_room(&claim("a.test", 6, 4), now));
        let first = DownloadId::new();
        admission.start(first, claim("a.test", 6, 4));
        assert!(
            admission.has_room(&claim("a.test", 6, 4), now),
            "two of six are left"
        );
        admission.start(DownloadId::new(), claim("a.test", 6, 4));
        assert!(!admission.has_room(&claim("a.test", 6, 4), now));
        assert!(
            admission.has_room(&claim("b.test", 6, 4), now),
            "another host has its own connections"
        );

        // Handed to the engine, the first file's chunks are counted by the limiter instead.
        admission.handed_over(first, now);
        assert!(!admission.has_room(&claim("a.test", 6, 4), now));
        let later = now + HANDOVER_GRACE + Duration::from_millis(1);
        admission.settle(std::collections::HashMap::new(), later);
        assert!(admission.has_room(&claim("a.test", 5, 4), later));
    }

    #[test]
    fn the_waits_of_the_last_pass_replace_the_earlier_ones() {
        let now = Instant::now();
        let mut admission = HostAdmission::default();
        let id = DownloadId::new();
        admission.settle([(id, "a.test".to_owned())].into(), now);
        assert_eq!(
            admission.waiting.get(&id).map(String::as_str),
            Some("a.test")
        );
        admission.settle(std::collections::HashMap::new(), now);
        assert!(admission.waiting.is_empty());
    }
}
